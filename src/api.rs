use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderName, HeaderValue};
use rust_i18n::t;
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
    pub variables: Vec<(String, String)>,
}

impl ApiItem {
    pub(crate) fn apply_variables(&mut self) {
        let vars = self.variables.clone();
        self.url = substitute_variables(&self.url, &vars);
        self.method = substitute_variables(&self.method, &vars);
        for (_, value) in &mut self.headers {
            *value = substitute_variables(value, &vars);
        }
        if let Some(body) = &mut self.body {
            *body = substitute_variables(body, &vars);
        }
    }
}

fn substitute_variables(text: &str, variables: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (key, value) in variables {
        let mut placeholder = String::from("{{");
        placeholder.push_str(key);
        placeholder.push_str("}}");
        result = result.replace(&placeholder, value);
    }
    result
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedCollection {
    pub(crate) apis: Vec<ApiItem>,
    pub(crate) variables: Vec<(String, String)>,
    pub(crate) secret_keys: BTreeSet<String>,
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
            ApiStatus::Running => t!("api.status.running").to_string(),
            ApiStatus::Done(s) => t!("api.status.status_code", code = s.to_string()).to_string(),
            ApiStatus::Failed(e) => t!("api.status.failed", error = e).to_string(),
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
struct PostmanVariable {
    key: String,
    value: String,
    #[serde(default, rename = "type")]
    var_type: String,
}

#[derive(Debug, Deserialize)]
struct PostmanCollection {
    #[serde(default)]
    variable: Vec<PostmanVariable>,
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

// OpenAPI support

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiDocument {
    #[serde(default)]
    openapi: String,
    #[serde(default)]
    info: OpenApiInfo,
    #[serde(default)]
    servers: Vec<OpenApiServer>,
    #[serde(default)]
    paths: BTreeMap<String, OpenApiPathItem>,
    #[serde(default)]
    components: OpenApiComponents,
    #[serde(default)]
    security: Option<Vec<BTreeMap<String, Vec<String>>>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiInfo {
    #[serde(default)]
    title: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiServer {
    #[serde(default)]
    url: String,
    #[serde(default)]
    variables: BTreeMap<String, OpenApiServerVariable>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiServerVariable {
    #[serde(default)]
    default: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiComponents {
    #[serde(default, rename = "securitySchemes")]
    security_schemes: BTreeMap<String, OpenApiSecurityScheme>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiSecurityScheme {
    #[serde(default, rename = "type")]
    scheme_type: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "in")]
    in_location: String,
    #[serde(default)]
    scheme: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiPathItem {
    #[serde(default)]
    parameters: Vec<OpenApiParameter>,
    #[serde(default)]
    get: Option<OpenApiOperation>,
    #[serde(default)]
    post: Option<OpenApiOperation>,
    #[serde(default)]
    put: Option<OpenApiOperation>,
    #[serde(default)]
    delete: Option<OpenApiOperation>,
    #[serde(default)]
    patch: Option<OpenApiOperation>,
    #[serde(default)]
    head: Option<OpenApiOperation>,
    #[serde(default)]
    options: Option<OpenApiOperation>,
    #[serde(default)]
    trace: Option<OpenApiOperation>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiOperation {
    #[serde(default, rename = "operationId")]
    operation_id: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: Vec<OpenApiParameter>,
    #[serde(default, rename = "requestBody")]
    request_body: Option<OpenApiRequestBody>,
    #[serde(default)]
    security: Option<Vec<BTreeMap<String, Vec<String>>>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiParameter {
    name: String,
    #[serde(default, rename = "in")]
    location: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    example: Option<serde_json::Value>,
    #[serde(default)]
    schema: Option<OpenApiSchema>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiRequestBody {
    #[serde(default)]
    content: BTreeMap<String, OpenApiMediaType>,
    #[serde(default)]
    required: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiMediaType {
    #[serde(default)]
    schema: Option<OpenApiSchema>,
    #[serde(default)]
    example: Option<serde_json::Value>,
    #[serde(default)]
    examples: BTreeMap<String, OpenApiExample>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiExample {
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct OpenApiSchema {
    #[serde(default, rename = "type")]
    schema_type: String,
    #[serde(default)]
    default: Option<serde_json::Value>,
    #[serde(default)]
    example: Option<serde_json::Value>,
    #[serde(default)]
    properties: Option<BTreeMap<String, OpenApiSchema>>,
    #[serde(default)]
    items: Option<Box<OpenApiSchema>>,
}

fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

fn schema_example(schema: &OpenApiSchema) -> Option<serde_json::Value> {
    if let Some(example) = &schema.example {
        return Some(example.clone());
    }
    if let Some(default) = &schema.default {
        return Some(default.clone());
    }
    let ty = schema.schema_type.as_str();
    if ty == "object" || schema.properties.is_some() {
        let mut obj = serde_json::Map::new();
        if let Some(props) = &schema.properties {
            for (k, v) in props {
                if let Some(ex) = schema_example(v) {
                    obj.insert(k.clone(), ex);
                }
            }
        }
        return Some(serde_json::Value::Object(obj));
    }
    if ty == "array" {
        if let Some(items) = &schema.items
            && let Some(ex) = schema_example(items)
        {
            return Some(serde_json::Value::Array(vec![ex]));
        }
        return Some(serde_json::Value::Array(vec![]));
    }
    match ty {
        "string" => Some(serde_json::Value::String(String::new())),
        "integer" | "number" => Some(serde_json::Value::Number(0.into())),
        "boolean" => Some(serde_json::Value::Bool(false)),
        _ => None,
    }
}

fn body_string_from_value(value: &serde_json::Value, content_type: &str) -> String {
    if is_json_content_type(Some(content_type)) {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    } else if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        value.to_string()
    }
}

fn media_body_string(media: &OpenApiMediaType, content_type: &str) -> Option<String> {
    if let Some(example) = &media.example {
        return Some(body_string_from_value(example, content_type));
    }
    if let Some(example) = media.examples.values().next().and_then(|e| e.value.as_ref()) {
        return Some(body_string_from_value(example, content_type));
    }
    if let Some(schema) = &media.schema
        && let Some(example) = schema_example(schema)
    {
        return Some(body_string_from_value(&example, content_type));
    }
    None
}

fn convert_braces(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
}

fn param_default(param: &OpenApiParameter) -> String {
    if let Some(example) = &param.example {
        return value_to_string(example);
    }
    if let Some(schema) = &param.schema {
        if let Some(example) = &schema.example {
            return value_to_string(example);
        }
        if let Some(default) = &schema.default {
            return value_to_string(default);
        }
    }
    String::new()
}

fn append_query(url: &mut String, first: &mut bool, name: &str, value: &str) {
    if *first {
        url.push('?');
        *first = false;
    } else {
        url.push('&');
    }
    url.push_str(name);
    url.push('=');
    url.push_str(value);
}

fn collect_operation_params(path_item: &OpenApiPathItem, operation: &OpenApiOperation) -> Vec<OpenApiParameter> {
    let mut params: Vec<OpenApiParameter> = path_item.parameters.clone();
    for p in &operation.parameters {
        if let Some(existing) = params.iter().position(|x| x.name == p.name) {
            params[existing] = p.clone();
        } else {
            params.push(p.clone());
        }
    }
    params
}

fn apply_parameters(
    params: &[OpenApiParameter],
    variables: &mut BTreeMap<String, String>,
    headers: &mut Vec<(String, String)>,
    url: &mut String,
) {
    let mut first_query = !url.contains('?');
    for param in params {
        let default = param_default(param);
        let placeholder = format!("{{{{{}}}}}" , param.name);
        variables.entry(param.name.clone()).or_insert(default);
        match param.location.as_str() {
            "query" => append_query(url, &mut first_query, &param.name, &placeholder),
            "header" => headers.push((param.name.clone(), placeholder)),
            _ => {}
        }
    }
}

fn apply_security(
    doc: &OpenApiDocument,
    operation: &OpenApiOperation,
    headers: &mut Vec<(String, String)>,
    url: &mut String,
    variables: &mut BTreeMap<String, String>,
    secret_keys: &mut BTreeSet<String>,
) {
    let empty: Vec<BTreeMap<String, Vec<String>>> = Vec::new();
    let security = operation.security.as_ref().or(doc.security.as_ref()).unwrap_or(&empty);
    let Some(requirement) = security.first() else { return };
    for scheme_name in requirement.keys() {
        let Some(scheme) = doc.components.security_schemes.get(scheme_name) else { continue };
        if scheme.scheme_type.eq_ignore_ascii_case("apiKey") {
            variables.entry(scheme_name.clone()).or_default();
            secret_keys.insert(scheme_name.clone());
            let placeholder = format!("{{{{{}}}}}" , scheme_name);
            match scheme.in_location.as_str() {
                "header" => headers.push((scheme.name.clone(), placeholder)),
                "query" => {
                    let mut first = !url.contains('?');
                    append_query(url, &mut first, &scheme.name, &placeholder);
                }
                _ => {}
            }
        }
    }
}

fn operation_name(operation: &OpenApiOperation, method: &str, path: &str) -> String {
    if !operation.operation_id.is_empty() {
        return operation.operation_id.clone();
    }
    if !operation.summary.is_empty() {
        return operation.summary.clone();
    }
    format!("{} {}", method.to_uppercase(), path)
}

fn base_url_from_servers(servers: &[OpenApiServer], variables: &mut BTreeMap<String, String>) -> String {
    let Some(server) = servers.first() else {
        return String::new();
    };
    for (name, var) in &server.variables {
        variables.entry(name.clone()).or_insert_with(|| var.default.clone());
    }
    server.url.clone()
}

fn parse_openapi_content(content: &str) -> Result<ParsedCollection> {
    let doc: OpenApiDocument = if let Ok(d) = serde_json::from_str(content) {
        d
    } else {
        serde_yaml::from_str(content).with_context(|| t!("api.openapi_parse_error").to_string())?
    };

    let mut variables: BTreeMap<String, String> = BTreeMap::new();
    let mut secret_keys = BTreeSet::new();

    let base_url = base_url_from_servers(&doc.servers, &mut variables);
    let base_url = base_url.trim_end_matches('/');

    let mut apis = Vec::new();
    for (path, path_item) in &doc.paths {
        let operations: Vec<(&str, &OpenApiOperation)> = [
            ("get", path_item.get.as_ref()),
            ("post", path_item.post.as_ref()),
            ("put", path_item.put.as_ref()),
            ("delete", path_item.delete.as_ref()),
            ("patch", path_item.patch.as_ref()),
            ("head", path_item.head.as_ref()),
            ("options", path_item.options.as_ref()),
            ("trace", path_item.trace.as_ref()),
        ]
        .into_iter()
        .filter_map(|(m, o)| o.map(|op| (m, op)))
        .collect();

        for (method, operation) in operations {
            let mut url = format!("{}{}", base_url, path);
            url = convert_braces(&url);
            let mut headers = Vec::new();

            let params = collect_operation_params(path_item, operation);
            apply_parameters(&params, &mut variables, &mut headers, &mut url);
            apply_security(&doc, operation, &mut headers, &mut url, &mut variables, &mut secret_keys);

            let body = if let Some(rb) = &operation.request_body {
                let mut selected: Option<(&String, &OpenApiMediaType)> = None;
                for (ct, media) in &rb.content {
                    if selected.is_none() {
                        selected = Some((ct, media));
                    }
                    if ct.contains("json") {
                        selected = Some((ct, media));
                        break;
                    }
                }
                if let Some((content_type, media)) = selected {
                    let body_str = media_body_string(media, content_type);
                    if body_str.is_some() && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                        headers.push(("Content-Type".to_string(), content_type.to_string()));
                    }
                    body_str
                } else {
                    None
                }
            } else {
                None
            };

            let name = operation_name(operation, method, path);
            apis.push(ApiItem {
                name,
                key: KeyBinding::Char('?'),
                method: method.to_uppercase(),
                url,
                headers,
                body,
                variables: Vec::new(),
            });
        }
    }

    let variables_vec: Vec<(String, String)> = variables.into_iter().collect();
    for api in &mut apis {
        api.variables = variables_vec.clone();
    }

    Ok(ParsedCollection {
        apis,
        variables: variables_vec,
        secret_keys,
    })
}

fn detect_format(content: &str) -> Result<&'static str> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        if value.get("openapi").is_some() {
            return Ok("openapi");
        }
        if value.get("info").and_then(|i| i.get("name")).is_some() && value.get("item").is_some() {
            return Ok("postman");
        }
    }
    if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(content) {
        if value.get("openapi").is_some() {
            return Ok("openapi");
        }
        if value.get("info").and_then(|i| i.get("name")).is_some() && value.get("item").is_some() {
            return Ok("postman");
        }
    }
    bail!("{}", t!("api.unknown_collection_format"))
}

fn assign_keybindings(apis: &mut [ApiItem]) {
    let keys: Vec<char> = ('a'..='z').chain('A'..='Z').chain('0'..='9').collect();
    for (i, api) in apis.iter_mut().enumerate() {
        if let Some(&c) = keys.get(i) {
            api.key = KeyBinding::Char(c);
        }
    }
}

fn parse_postman_content(content: &str) -> Result<ParsedCollection> {
    let collection: PostmanCollection = if let Ok(c) = serde_json::from_str(content) {
        c
    } else {
        serde_yaml::from_str(content).with_context(|| t!("api.postman_parse_error").to_string())?
    };

    let mut secret_keys = BTreeSet::new();
    let variables: Vec<(String, String)> = collection
        .variable
        .into_iter()
        .map(|v| {
            if v.var_type.eq_ignore_ascii_case("secret") {
                secret_keys.insert(v.key.clone());
            }
            (v.key, v.value)
        })
        .collect();

    let mut apis = Vec::new();
    for item in collection.item {
        extract_item("", item, &variables, &mut apis);
    }

    Ok(ParsedCollection { apis, variables, secret_keys })
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

fn extract_item(
    prefix: &str,
    item: PostmanItem,
    variables: &[(String, String)],
    apis: &mut Vec<ApiItem>,
) {
    let name = if prefix.is_empty() {
        item.name
    } else {
        format!("{} / {}", prefix, item.name)
    };

    if !item.item.is_empty() {
        for child in item.item {
            extract_item(&name, child, variables, apis);
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
                    variables: variables.to_vec(),
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
                    variables: variables.to_vec(),
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

pub(crate) fn parse_collection(path: &Path) -> Result<ParsedCollection> {
    let content = std::fs::read_to_string(path)
        .with_context(|| t!("api.read_collection_error", path = path.display().to_string()).to_string())?;
    let format = detect_format(&content)?;
    let mut parsed = match format {
        "openapi" => parse_openapi_content(&content),
        "postman" => parse_postman_content(&content),
        _ => bail!("{}", t!("api.unknown_collection_format")),
    }
    .with_context(|| t!("api.parse_collection_error", path = path.display().to_string()).to_string())?;
    assign_keybindings(&mut parsed.apis);
    Ok(parsed)
}

pub(crate) async fn run_api_request(
    tab: usize,
    id: usize,
    item: ApiItem,
    client: reqwest::Client,
    tx: UnboundedSender<AppEvent>,
) {
    if let Err(e) = run_api_request_inner(tab, id, item, client, tx.clone()).await {
        let _ = tx.send(AppEvent::ApiError { tab, id, error: e.to_string() });
    }
}

async fn run_api_request_inner(
    tab: usize,
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

    let mut headers_text = t!("api.http_status_line", status = status.to_string()).to_string();
    for (name, value) in response.headers() {
        if let Ok(v) = value.to_str() {
            headers_text.push_str(&format!("{}: {}\n", name, v));
        }
    }

    let body = response.text().await.unwrap_or_default();

    let _ = tx.send(AppEvent::ApiResponse { tab, id, status, headers: headers_text, body });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn parse_openapi_yaml() {
        let path = manifest_dir().join("test_openapi.yaml");
        let parsed = parse_collection(&path).expect("parse openapi yaml");
        assert_eq!(parsed.apis.len(), 2);
        assert!(parsed.secret_keys.contains("apiKey"));
        let keys: Vec<&str> = parsed.variables.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"apiKey"));
        assert!(keys.contains(&"foo"));
    }

    #[test]
    fn parse_openapi_json() {
        let path = manifest_dir().join("test_openapi.json");
        let parsed = parse_collection(&path).expect("parse openapi json");
        assert_eq!(parsed.apis.len(), 2);
        assert!(parsed.secret_keys.contains("apiKey"));
    }
}
