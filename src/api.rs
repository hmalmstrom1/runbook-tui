use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Context, Result};
use reqwest::header::{HeaderName, HeaderValue};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{AppEvent, MAX_LOG_LINES};
use crate::keybinding::KeyBinding;
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub(crate) struct ApiItem {
    pub name: String,
    pub key: KeyBinding,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum ApiStatus {
    Running,
    Done(u16),
    Failed(String),
}

impl ApiStatus {
    pub fn indicator(&self, theme: &Theme) -> (&'static str, ratatui::style::Style) {
        match self {
            ApiStatus::Running => ("●", theme.running),
            ApiStatus::Done(s) if (200..300).contains(s) => ("✓", theme.success),
            ApiStatus::Done(_) => ("✗", theme.error),
            ApiStatus::Failed(_) => ("✗", theme.error),
        }
    }

    pub fn label(&self) -> String {
        match self {
            ApiStatus::Running => "running".to_string(),
            ApiStatus::Done(s) => format!("status {}", s),
            ApiStatus::Failed(e) => format!("failed: {}", e),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ApiRequest {
    pub id: usize,
    pub method: String,
    pub url: String,
    pub status: ApiStatus,
    pub request_headers: String,
    pub request_body: VecDeque<String>,
    pub request_content_type: Option<String>,
    pub headers: String,
    pub body: VecDeque<String>,
    pub response_content_type: Option<String>,
}

impl ApiRequest {
    pub fn push_line(&mut self, line: String) {
        if self.body.len() >= MAX_LOG_LINES {
            self.body.pop_front();
        }
        self.body.push_back(line);
    }

    pub fn set_response(&mut self, status: u16, headers: String, body: String) {
        self.status = ApiStatus::Done(status);
        self.headers = headers.clone();
        self.response_content_type = parse_content_type(&headers);
        self.body.clear();
        let text = format_body(&body, self.response_content_type.as_deref());
        for line in text.lines() {
            self.push_line(line.to_string());
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.status = ApiStatus::Failed(error);
    }
}

#[derive(Debug, Deserialize)]
struct PostmanCollection {
    item: Vec<PostmanItem>,
}

#[derive(Debug, Deserialize)]
struct PostmanItem {
    name: String,
    #[serde(default)]
    item: Vec<PostmanItem>,
    request: Option<PostmanRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PostmanRequest {
    String(String),
    Object(PostmanRequestObject),
}

#[derive(Debug, Deserialize)]
struct PostmanRequestObject {
    #[serde(default = "default_method")]
    method: String,
    url: Option<PostmanUrl>,
    #[serde(default)]
    header: Vec<PostmanHeader>,
    body: Option<PostmanBody>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PostmanUrl {
    Raw(String),
    Object { raw: Option<String> },
}

#[derive(Debug, Deserialize)]
struct PostmanHeader {
    key: String,
    value: String,
    #[serde(default)]
    disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PostmanBody {
    #[serde(default)]
    mode: String,
    raw: Option<String>,
}

fn default_method() -> String {
    "GET".to_string()
}

fn url_string(url: Option<PostmanUrl>) -> String {
    match url {
        Some(PostmanUrl::Raw(s)) => s,
        Some(PostmanUrl::Object { raw: Some(s) }) => s,
        _ => String::new(),
    }
}

fn extract_item(prefix: &str, item: PostmanItem, apis: &mut Vec<ApiItem>) {
    let name = if prefix.is_empty() {
        item.name
    } else {
        format!("{} / {}", prefix, item.name)
    };

    if !item.item.is_empty() {
        for child in item.item {
            extract_item(&name, child, apis);
        }
        return;
    }

    if let Some(request) = item.request {
        match request {
            PostmanRequest::String(url) => {
                apis.push(ApiItem {
                    name: name.clone(),
                    key: KeyBinding::Char('?'),
                    method: "GET".to_string(),
                    url,
                    headers: Vec::new(),
                    body: None,
                });
            }
            PostmanRequest::Object(obj) => {
                let method = obj.method.to_uppercase();
                let url = url_string(obj.url);
                let mut headers = Vec::new();
                for h in obj.header {
                    if h.disabled.unwrap_or(false) {
                        continue;
                    }
                    if h.key.trim().is_empty() {
                        continue;
                    }
                    headers.push((h.key, h.value));
                }
                let body = obj.body.and_then(|b| {
                    if b.mode == "raw" {
                        b.raw
                    } else {
                        None
                    }
                });
                apis.push(ApiItem {
                    name,
                    key: KeyBinding::Char('?'),
                    method,
                    url,
                    headers,
                    body,
                });
            }
        }
    }
}

pub(crate) fn parse_content_type(headers: &str) -> Option<String> {
    headers
        .lines()
        .find_map(|line| {
            let mut parts = line.splitn(2, ':');
            let key = parts.next()?;
            if key.eq_ignore_ascii_case("content-type") {
                parts.next().map(|v| v.trim().to_lowercase())
            } else {
                None
            }
        })
}

fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type
        .map(|ct| ct.contains("json") || ct.contains("javascript"))
        .unwrap_or(false)
}

pub(crate) fn format_body(body: &str, content_type: Option<&str>) -> String {
    if is_json_content_type(content_type)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(body)
    {
        return serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string());
    }
    body.to_string()
}

pub(crate) fn parse_collection(path: &Path) -> Result<Vec<ApiItem>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read collection {}", path.display()))?;
    let collection: PostmanCollection = serde_json::from_str(&content)
        .with_context(|| "Failed to parse collection as JSON")?;

    let mut apis = Vec::new();
    for item in collection.item {
        extract_item("", item, &mut apis);
    }

    let keys: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    for (i, api) in apis.iter_mut().enumerate() {
        if let Some(&c) = keys.get(i) {
            api.key = KeyBinding::Char(c);
        }
    }

    Ok(apis)
}

pub(crate) async fn run_api_request(
    id: usize,
    item: ApiItem,
    client: reqwest::Client,
    tx: UnboundedSender<AppEvent>,
) {
    if let Err(e) = run_api_request_inner(id, item, client, tx.clone()).await {
        let _ = tx.send(AppEvent::ApiError { id, error: e.to_string() });
    }
}

async fn run_api_request_inner(
    id: usize,
    item: ApiItem,
    client: reqwest::Client,
    tx: UnboundedSender<AppEvent>,
) -> Result<()> {
    let method = reqwest::Method::from_bytes(item.method.as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let mut builder = client.request(method, &item.url);

    for (k, v) in &item.headers {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            builder = builder.header(name, value);
        }
    }

    if let Some(body) = item.body {
        builder = builder.body(body);
    }

    let response = builder.send().await?;
    let status = response.status().as_u16();

    let mut headers_text = format!("HTTP {}\n", status);
    for (name, value) in response.headers() {
        if let Ok(v) = value.to_str() {
            headers_text.push_str(&format!("{}: {}\n", name, v));
        }
    }

    let body = response.text().await.unwrap_or_default();

    let _ = tx.send(AppEvent::ApiResponse { id, status, headers: headers_text, body });
    Ok(())
}
