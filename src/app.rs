use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{self, ApiItem, ApiRequest, ApiStatus, format_body, run_api_request};
use crate::config::{read_config, Command};
use crate::process::run_process;
use crate::theme::Theme;
use crate::ui::colorize_body;

pub(crate) const MAX_LOG_LINES: usize = 50_000;

#[derive(Debug, Clone)]
pub(crate) enum ProcessStatus {
    Running,
    Exited(i32),
    Failed(String),
}

impl ProcessStatus {
    pub(crate) fn indicator(&self, theme: &Theme) -> (&'static str, ratatui::style::Style) {
        match self {
            ProcessStatus::Running => ("●", theme.running),
            ProcessStatus::Exited(0) => ("✓", theme.success),
            ProcessStatus::Exited(_) => ("✗", theme.error),
            ProcessStatus::Failed(_) => ("✗", theme.error),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            ProcessStatus::Running => "running".to_string(),
            ProcessStatus::Exited(c) => format!("exit {}", c),
            ProcessStatus::Failed(e) => format!("failed: {}", e),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Process {
    pub(crate) id: usize,
    pub(crate) title: String,
    pub(crate) _command: String,
    pub(crate) status: ProcessStatus,
    pub(crate) _log_path: PathBuf,
    pub(crate) output: VecDeque<String>,
    pub(crate) _start: Instant,
}

impl Process {
    pub(crate) fn push_line(&mut self, line: String) {
        if self.output.len() >= MAX_LOG_LINES {
            self.output.pop_front();
        }
        self.output.push_back(line);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputMode {
    Normal,
    Search,
    Import,
    VariableEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Commands,
    Processes,
    Output,
    RequestBody,
    ResponseBody,
    Variables,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppMode {
    Runbook,
    Api,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportEntry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
}

pub(crate) struct App {
    pub(crate) app_mode: AppMode,
    pub(crate) client: reqwest::Client,
    pub(crate) commands: Vec<Command>,
    pub(crate) search: String,
    pub(crate) filtered: Vec<usize>,
    pub(crate) selected_command: usize,
    pub(crate) command_state: ListState,
    pub(crate) apis: Vec<ApiItem>,
    pub(crate) filtered_apis: Vec<usize>,
    pub(crate) selected_api: usize,
    pub(crate) api_state: ListState,
    pub(crate) input_mode: InputMode,
    pub(crate) import_path: PathBuf,
    pub(crate) import_filter: String,
    pub(crate) import_cwd: PathBuf,
    pub(crate) import_entries: Vec<ImportEntry>,
    pub(crate) import_state: ListState,
    pub(crate) import_message: String,
    pub(crate) import_error: bool,
    pub(crate) focus: Focus,
    pub(crate) processes: Vec<Process>,
    pub(crate) process_state: ListState,
    pub(crate) api_requests: Vec<ApiRequest>,
    pub(crate) request_state: ListState,
    pub(crate) log_state: ListState,
    pub(crate) log_follow: bool,
    pub(crate) log_area_height: u16,
    pub(crate) request_body_state: ListState,
    pub(crate) request_body_area_height: u16,
    pub(crate) request_body_area_width: u16,
    pub(crate) id_counter: usize,
    pub(crate) show_help: bool,
    pub(crate) quit: bool,
    pub(crate) theme: Theme,
    pub(crate) variables: Vec<(String, String)>,
    pub(crate) variable_state: ListState,
    pub(crate) variable_edit_index: Option<usize>,
    pub(crate) variable_edit_value: String,
}

impl App {
    pub(crate) fn new(commands: Vec<Command>, client: reqwest::Client) -> Self {
        let mut app = Self {
            app_mode: AppMode::Runbook,
            client,
            commands,
            search: String::new(),
            filtered: Vec::new(),
            selected_command: 0,
            command_state: ListState::default(),
            apis: Vec::new(),
            filtered_apis: Vec::new(),
            selected_api: 0,
            api_state: ListState::default(),
            input_mode: InputMode::Normal,
            import_path: PathBuf::new(),
            import_filter: String::new(),
            import_cwd: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            import_entries: Vec::new(),
            import_state: ListState::default(),
            import_message: String::new(),
            import_error: false,
            focus: Focus::Commands,
            processes: Vec::new(),
            process_state: ListState::default(),
            api_requests: Vec::new(),
            request_state: ListState::default(),
            log_state: ListState::default(),
            log_follow: true,
            log_area_height: 0,
            request_body_state: ListState::default(),
            request_body_area_height: 0,
            request_body_area_width: 0,
            id_counter: 0,
            show_help: false,
            quit: false,
            theme: Theme::default(),
            variables: Vec::new(),
            variable_state: ListState::default(),
            variable_edit_index: None,
            variable_edit_value: String::new(),
        };
        app.update_filtered();
        app
    }

    pub(crate) fn new_api(
        apis: Vec<ApiItem>,
        client: reqwest::Client,
        variables: Vec<(String, String)>,
    ) -> Self {
        let mut app = Self {
            app_mode: AppMode::Api,
            client,
            commands: Vec::new(),
            search: String::new(),
            filtered: Vec::new(),
            selected_command: 0,
            command_state: ListState::default(),
            apis,
            filtered_apis: Vec::new(),
            selected_api: 0,
            api_state: ListState::default(),
            input_mode: InputMode::Normal,
            import_path: PathBuf::new(),
            import_filter: String::new(),
            import_cwd: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            import_entries: Vec::new(),
            import_state: ListState::default(),
            import_message: String::new(),
            import_error: false,
            focus: Focus::Commands,
            processes: Vec::new(),
            process_state: ListState::default(),
            api_requests: Vec::new(),
            request_state: ListState::default(),
            log_state: ListState::default(),
            log_follow: true,
            log_area_height: 0,
            request_body_state: ListState::default(),
            request_body_area_height: 0,
            request_body_area_width: 0,
            id_counter: 0,
            show_help: false,
            quit: false,
            theme: Theme::default(),
            variables,
            variable_state: ListState::default(),
            variable_edit_index: None,
            variable_edit_value: String::new(),
        };
        app.update_filtered();
        app.variable_state.select(Some(0));
        app
    }

    pub(crate) fn update_filtered(&mut self) {
        let q = self.search.to_lowercase();
        match self.app_mode {
            AppMode::Runbook => {
                self.filtered = self
                    .commands
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.title.to_lowercase().contains(&q))
                    .map(|(i, _)| i)
                    .collect();
                if self.selected_command >= self.filtered.len() {
                    self.selected_command = self.filtered.len().saturating_sub(1);
                }
                self.command_state.select(Some(self.selected_command));
            }
            AppMode::Api => {
                self.filtered_apis = self
                    .apis
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.name.to_lowercase().contains(&q))
                    .map(|(i, _)| i)
                    .collect();
                if self.selected_api >= self.filtered_apis.len() {
                    self.selected_api = self.filtered_apis.len().saturating_sub(1);
                }
                self.api_state.select(Some(self.selected_api));
            }
        }
    }

    pub(crate) fn select_next_command(&mut self) {
        if !self.filtered.is_empty() {
            self.selected_command = (self.selected_command + 1) % self.filtered.len();
            self.command_state.select(Some(self.selected_command));
        }
    }

    pub(crate) fn select_prev_command(&mut self) {
        if !self.filtered.is_empty() {
            self.selected_command = self
                .selected_command
                .checked_sub(1)
                .unwrap_or(self.filtered.len() - 1);
            self.command_state.select(Some(self.selected_command));
        }
    }

    pub(crate) fn select_next_api(&mut self) {
        if !self.filtered_apis.is_empty() {
            self.selected_api = (self.selected_api + 1) % self.filtered_apis.len();
            self.api_state.select(Some(self.selected_api));
        }
    }

    pub(crate) fn select_prev_api(&mut self) {
        if !self.filtered_apis.is_empty() {
            self.selected_api = self
                .selected_api
                .checked_sub(1)
                .unwrap_or(self.filtered_apis.len() - 1);
            self.api_state.select(Some(self.selected_api));
        }
    }

    pub(crate) fn select_next_variable(&mut self) {
        if !self.variables.is_empty() {
            let i = self.variable_state.selected().unwrap_or(0);
            let next = (i + 1) % self.variables.len();
            self.variable_state.select(Some(next));
        }
    }

    pub(crate) fn select_prev_variable(&mut self) {
        if !self.variables.is_empty() {
            let i = self.variable_state.selected().unwrap_or(0);
            let prev = i.checked_sub(1).unwrap_or(self.variables.len() - 1);
            self.variable_state.select(Some(prev));
        }
    }

    pub(crate) fn start_variable_edit(&mut self) {
        if let Some(i) = self.variable_state.selected()
            && let Some((_, value)) = self.variables.get(i)
        {
            self.variable_edit_index = Some(i);
            self.variable_edit_value = value.clone();
            self.input_mode = InputMode::VariableEdit;
        }
    }

    pub(crate) fn confirm_variable_edit(&mut self) {
        if let Some(i) = self.variable_edit_index
            && let Some(entry) = self.variables.get_mut(i)
        {
            entry.1 = self.variable_edit_value.clone();
        }
        self.cancel_variable_edit();
    }

    pub(crate) fn cancel_variable_edit(&mut self) {
        self.variable_edit_index = None;
        self.variable_edit_value.clear();
        self.input_mode = InputMode::Normal;
    }

    pub(crate) fn variable_edit_push(&mut self, c: char) {
        self.variable_edit_value.push(c);
    }

    pub(crate) fn variable_edit_pop(&mut self) {
        self.variable_edit_value.pop();
    }

    pub(crate) fn select_next_process(&mut self) {
        if !self.processes.is_empty() {
            let i = self.process_state.selected().unwrap_or(0);
            let next = (i + 1) % self.processes.len();
            self.process_state.select(Some(next));
            self.log_follow = true;
        }
    }

    pub(crate) fn select_prev_process(&mut self) {
        if !self.processes.is_empty() {
            let i = self.process_state.selected().unwrap_or(0);
            let prev = i.checked_sub(1).unwrap_or(self.processes.len() - 1);
            self.process_state.select(Some(prev));
            self.log_follow = true;
        }
    }

    pub(crate) fn select_next_request(&mut self) {
        if !self.api_requests.is_empty() {
            let i = self.request_state.selected().unwrap_or(0);
            let next = (i + 1) % self.api_requests.len();
            self.request_state.select(Some(next));
            self.log_follow = true;
        }
    }

    pub(crate) fn select_prev_request(&mut self) {
        if !self.api_requests.is_empty() {
            let i = self.request_state.selected().unwrap_or(0);
            let prev = i.checked_sub(1).unwrap_or(self.api_requests.len() - 1);
            self.request_state.select(Some(prev));
            self.log_follow = true;
        }
    }

    pub(crate) fn search_push(&mut self, c: char) {
        self.search.push(c);
        if self.app_mode == AppMode::Runbook {
            self.selected_command = 0;
        } else {
            self.selected_api = 0;
        }
        self.update_filtered();
    }

    pub(crate) fn search_pop(&mut self) {
        self.search.pop();
        if self.app_mode == AppMode::Runbook {
            self.selected_command = 0;
        } else {
            self.selected_api = 0;
        }
        self.update_filtered();
    }

    pub(crate) fn start_import(&mut self) {
        self.input_mode = InputMode::Import;
        self.import_path = PathBuf::new();
        self.import_filter.clear();
        self.import_cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.import_message = "Type to filter, ↑/↓ to select, Enter to open".to_string();
        self.import_error = false;
        self.refresh_import_entries();
        self.import_state.select(Some(0));
    }

    pub(crate) fn import_filter_push(&mut self, c: char) {
        self.import_filter.push(c);
        self.import_error = false;
        self.refresh_import_entries();
        self.import_state.select(Some(0));
    }

    pub(crate) fn import_filter_pop(&mut self) {
        if self.import_filter.pop().is_some() {
            self.import_error = false;
            self.refresh_import_entries();
            self.import_state.select(Some(0));
        }
    }

    pub(crate) fn refresh_import_entries(&mut self) {
        self.import_entries.clear();
        let filter = self.import_filter.to_lowercase();

        if filter.is_empty()
            && let Some(parent) = self.import_cwd.parent()
        {
            self.import_entries.push(ImportEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
            });
        }

        match fs::read_dir(&self.import_cwd) {
            Ok(entries) => {
                let mut dirs = Vec::new();
                let mut files = Vec::new();
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let is_dir = match entry.file_type() {
                        Ok(t) => t.is_dir(),
                        Err(_) => path.is_dir(),
                    };
                    if name.starts_with('.') && filter.is_empty() && name != "." && name != ".." {
                        continue;
                    }
                    let item = ImportEntry {
                        name,
                        path,
                        is_dir,
                    };
                    if is_dir {
                        dirs.push(item);
                    } else {
                        files.push(item);
                    }
                }
                dirs.sort_by_key(|a| a.name.to_lowercase());
                files.sort_by_key(|a| a.name.to_lowercase());
                self.import_entries.append(&mut dirs);
                self.import_entries.append(&mut files);
                self.import_message = format!("{} items", self.import_entries.len());
                self.import_error = false;
            }
            Err(e) => {
                self.import_message = format!("Error: {}", e);
                self.import_error = true;
            }
        }
    }

    pub(crate) fn select_prev_import_entry(&mut self) {
        if !self.import_entries.is_empty() {
            let i = self.import_state.selected().unwrap_or(0);
            let prev = i.checked_sub(1).unwrap_or(self.import_entries.len() - 1);
            self.import_state.select(Some(prev));
        }
    }

    pub(crate) fn select_next_import_entry(&mut self) {
        if !self.import_entries.is_empty() {
            let i = self.import_state.selected().unwrap_or(0);
            let next = (i + 1) % self.import_entries.len();
            self.import_state.select(Some(next));
        }
    }

    pub(crate) fn import_enter_selected(&mut self) {
        if let Some(idx) = self.import_state.selected()
            && let Some(entry) = self.import_entries.get(idx)
        {
            if entry.is_dir {
                self.import_cwd = entry.path.clone();
                self.import_filter.clear();
                self.refresh_import_entries();
                self.import_state.select(Some(0));
            } else {
                self.import_path = entry.path.clone();
                self.confirm_import();
            }
        }
    }

    pub(crate) fn confirm_import(&mut self) {
        let path = self.import_path.clone();
        let result = if path.extension().and_then(|e| e.to_str()) == Some("json") {
            api::parse_collection(&path).map(|parsed| {
                let client = self.client.clone();
                *self = Self::new_api(parsed.apis, client, parsed.variables);
            })
        } else {
            read_config(&path).map(|commands| {
                let client = self.client.clone();
                *self = Self::new(commands, client);
            })
        };
        if let Err(e) = result {
            self.import_message = format!("Error: {}", e);
            self.import_error = true;
        }
    }

    pub(crate) fn scroll_log_up(&mut self, amount: usize) {
        if let Some(selected) = self.log_state.selected() {
            self.log_follow = false;
            self.log_state.select(Some(selected.saturating_sub(amount)));
        }
    }

    pub(crate) fn scroll_log_down(&mut self, amount: usize) {
        if let Some(selected) = self.log_state.selected() {
            self.log_state.select(Some(selected.saturating_add(amount)));
        }
    }

    pub(crate) fn scroll_log_top(&mut self) {
        self.log_follow = false;
        self.log_state.select(Some(0));
    }

    pub(crate) fn scroll_log_bottom(&mut self) {
        self.log_follow = true;
    }

    pub(crate) fn scroll_request_body_up(&mut self, amount: usize) {
        if let Some(selected) = self.request_body_state.selected() {
            self.request_body_state.select(Some(selected.saturating_sub(amount)));
        }
    }

    pub(crate) fn scroll_request_body_down(&mut self, amount: usize) {
        if let Some(selected) = self.request_body_state.selected() {
            self.request_body_state.select(Some(selected.saturating_add(amount)));
        }
    }

    pub(crate) fn scroll_request_body_top(&mut self) {
        self.request_body_state.select(Some(0));
    }

    pub(crate) fn scroll_request_body_bottom(&mut self) {
        if let Some(r) = self.selected_request() {
            let count = colorize_body(
                r.request_content_type.as_deref(),
                &r.request_body,
                self.request_body_area_width,
                &self.theme,
            )
            .len();
            if count > 0 {
                self.request_body_state.select(Some(count - 1));
            }
        }
    }

    pub(crate) fn cycle_focus(&mut self) {
        self.input_mode = InputMode::Normal;
        self.focus = match (self.app_mode, self.focus) {
            (AppMode::Runbook, Focus::Commands) => Focus::Processes,
            (AppMode::Runbook, Focus::Processes) => Focus::Output,
            (AppMode::Runbook, Focus::Output) => Focus::Commands,
            (AppMode::Api, Focus::Commands) => Focus::Variables,
            (AppMode::Api, Focus::Variables) => Focus::Processes,
            (AppMode::Api, Focus::Processes) => Focus::RequestBody,
            (AppMode::Api, Focus::RequestBody) => Focus::ResponseBody,
            (AppMode::Api, Focus::ResponseBody) => Focus::Commands,
            _ => Focus::Commands,
        };
        if self.focus == Focus::Processes {
            match self.app_mode {
                AppMode::Runbook => {
                    if !self.processes.is_empty() && self.process_state.selected().is_none() {
                        self.process_state.select(Some(self.processes.len() - 1));
                        self.log_follow = true;
                    }
                }
                AppMode::Api => {
                    if !self.api_requests.is_empty() && self.request_state.selected().is_none() {
                        self.request_state.select(Some(self.api_requests.len() - 1));
                        self.log_follow = true;
                    }
                }
            }
        }
    }

    pub(crate) fn spawn(&mut self, command_idx: usize, tx: UnboundedSender<AppEvent>) {
        let cmd = &self.commands[command_idx];
        let id = self.id_counter;
        self.id_counter += 1;
        let title = cmd.title.clone();
        let shell = cmd.command.clone();
        let log_dir = env::temp_dir().join("runbook-tui");
        let log_path = log_dir.join(format!("{}.log", id));

        let process = Process {
            id,
            title,
            _command: shell.clone(),
            status: ProcessStatus::Running,
            _log_path: log_path.clone(),
            output: VecDeque::new(),
            _start: Instant::now(),
        };
        self.processes.push(process);
        let process_idx = self.processes.len() - 1;
        self.process_state.select(Some(process_idx));
        self.log_state = ListState::default();
        self.log_follow = true;

        tokio::spawn(run_process(id, shell, log_path, tx));
    }

    pub(crate) fn spawn_api(&mut self, api_idx: usize, tx: UnboundedSender<AppEvent>) {
        let mut item = self.apis[api_idx].clone();
        item.variables = self.variables.clone();
        item.apply_variables();
        let id = self.id_counter;
        self.id_counter += 1;
        let request_headers = item
            .headers
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect::<Vec<_>>()
            .join("\n");

        let request_content_type = item
            .headers
            .iter()
            .find_map(|(k, v)| {
                if k.eq_ignore_ascii_case("content-type") {
                    Some(v.to_lowercase())
                } else {
                    None
                }
            });

        let mut request_body = VecDeque::new();
        if let Some(body) = &item.body {
            let formatted = format_body(body, request_content_type.as_deref());
            for line in formatted.lines() {
                request_body.push_back(line.to_string());
            }
        }

        let request = ApiRequest {
            id,
            method: item.method.clone(),
            url: item.url.clone(),
            status: ApiStatus::Running,
            request_headers,
            request_body,
            request_content_type,
            headers: String::new(),
            body: VecDeque::new(),
            response_content_type: None,
        };
        self.api_requests.push(request);
        let request_idx = self.api_requests.len() - 1;
        self.request_state.select(Some(request_idx));
        self.log_state = ListState::default();
        self.log_follow = true;

        tokio::spawn(run_api_request(id, item, self.client.clone(), tx));
    }

    pub(crate) fn run_selected(&mut self, tx: UnboundedSender<AppEvent>) {
        match self.app_mode {
            AppMode::Runbook => {
                if let Some(&idx) = self.filtered.get(self.selected_command) {
                    self.spawn(idx, tx);
                }
            }
            AppMode::Api => {
                if let Some(&idx) = self.filtered_apis.get(self.selected_api) {
                    self.spawn_api(idx, tx);
                }
            }
        }
    }

    pub(crate) fn try_keybinding(&mut self, key: &KeyEvent, tx: UnboundedSender<AppEvent>) {
        match self.app_mode {
            AppMode::Runbook => {
                for (i, cmd) in self.commands.iter().enumerate() {
                    if cmd.key.matches(key) {
                        self.spawn(i, tx);
                        return;
                    }
                }
            }
            AppMode::Api => {
                for (i, api) in self.apis.iter().enumerate() {
                    if api.key.matches(key) {
                        self.spawn_api(i, tx);
                        return;
                    }
                }
            }
        }
    }

    pub(crate) fn selected_process(&self) -> Option<&Process> {
        self.process_state
            .selected()
            .and_then(|i| self.processes.get(i))
    }

    pub(crate) fn selected_request(&self) -> Option<&ApiRequest> {
        self.request_state
            .selected()
            .and_then(|i| self.api_requests.get(i))
    }

    pub(crate) fn push_process_line(&mut self, id: usize, line: String) {
        if let Some(p) = self.processes.iter_mut().find(|p| p.id == id) {
            p.push_line(line);
        }
    }

    pub(crate) fn set_process_exit(&mut self, id: usize, code: Option<i32>) {
        let code = code.unwrap_or(-1);
        if let Some(p) = self.processes.iter_mut().find(|p| p.id == id) {
            p.status = ProcessStatus::Exited(code);
        }
    }

    pub(crate) fn set_process_error(&mut self, id: usize, error: String) {
        if let Some(p) = self.processes.iter_mut().find(|p| p.id == id) {
            p.status = ProcessStatus::Failed(error);
        }
    }

    pub(crate) fn push_api_response(&mut self, id: usize, status: u16, headers: String, body: String) {
        if let Some(r) = self.api_requests.iter_mut().find(|r| r.id == id) {
            r.set_response(status, headers, body);
        }
    }

    pub(crate) fn set_api_error(&mut self, id: usize, error: String) {
        if let Some(r) = self.api_requests.iter_mut().find(|r| r.id == id) {
            r.set_error(error);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AppEvent {
    Tick,
    Input(Event),
    ProcessLine { id: usize, line: String },
    ProcessExit { id: usize, code: Option<i32> },
    ProcessError { id: usize, error: String },
    ApiResponse { id: usize, status: u16, headers: String, body: String },
    ApiError { id: usize, error: String },
}

pub(crate) fn read_input(running: Arc<AtomicBool>, tx: UnboundedSender<AppEvent>) {
    while running.load(Ordering::Relaxed) {
        if event::poll(Duration::from_millis(100)).unwrap_or(false)
            && let Ok(evt) = event::read()
            && tx.send(AppEvent::Input(evt)).is_err()
        {
            break;
        }
    }
}

pub(crate) fn handle_event(app: &mut App, event: AppEvent, tx: UnboundedSender<AppEvent>) {
    match event {
        AppEvent::Tick => {}
        AppEvent::Input(Event::Key(key)) => handle_key(app, key, tx),
        AppEvent::Input(_) => {}
        AppEvent::ProcessLine { id, line } => app.push_process_line(id, line),
        AppEvent::ProcessExit { id, code } => app.set_process_exit(id, code),
        AppEvent::ProcessError { id, error } => app.set_process_error(id, error),
        AppEvent::ApiResponse { id, status, headers, body } => {
            app.push_api_response(id, status, headers, body);
        }
        AppEvent::ApiError { id, error } => app.set_api_error(id, error),
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent, tx: UnboundedSender<AppEvent>) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return;
    }
    if app.show_help {
        if key.code == KeyCode::Char('?')
            || key.code == KeyCode::Esc
            || key.code == KeyCode::Char('q')
        {
            app.show_help = false;
        }
        return;
    }
    if key.code == KeyCode::Char('?') && app.input_mode == InputMode::Normal {
        app.show_help = true;
        return;
    }
    if app.input_mode == InputMode::Import {
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            app.input_mode = InputMode::Normal;
        } else {
            handle_import_key(app, key);
        }
        return;
    }
    if app.input_mode == InputMode::VariableEdit {
        match key.code {
            KeyCode::Enter => app.confirm_variable_edit(),
            KeyCode::Esc => app.cancel_variable_edit(),
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                app.variable_edit_push(c);
            }
            KeyCode::Backspace => app.variable_edit_pop(),
            _ => {}
        }
        return;
    }
    if key.code == KeyCode::Tab {
        app.cycle_focus();
        return;
    }
    if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.start_import();
        return;
    }
    if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
        handle_ctrl_n(app);
        return;
    }
    if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
        handle_ctrl_p(app);
        return;
    }

    match (app.app_mode, app.focus) {
        (AppMode::Runbook, Focus::Commands) => handle_commands_key(app, key, tx),
        (AppMode::Runbook, Focus::Processes) => handle_processes_key(app, key),
        (AppMode::Api, Focus::Commands) => handle_apis_key(app, key, tx),
        (AppMode::Api, Focus::Variables) => handle_variables_key(app, key),
        (AppMode::Api, Focus::Processes) => handle_requests_key(app, key),
        (AppMode::Api, Focus::RequestBody) => handle_request_body_key(app, key),
        (AppMode::Api, Focus::ResponseBody) => handle_output_key(app, key),
        (_, Focus::Output) => handle_output_key(app, key),
        _ => {}
    }
}

fn handle_variables_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => app.select_prev_variable(),
        KeyCode::Down => app.select_next_variable(),
        KeyCode::Enter => app.start_variable_edit(),
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.select_next_variable();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.select_prev_variable();
        }
        _ => {}
    }
}

pub(crate) fn handle_ctrl_n(app: &mut App) {
    match (app.app_mode, app.focus) {
        (AppMode::Runbook, Focus::Commands) => app.select_next_command(),
        (AppMode::Runbook, Focus::Processes) => app.select_next_process(),
        (AppMode::Api, Focus::Commands) => app.select_next_api(),
        (AppMode::Api, Focus::Variables) => app.select_next_variable(),
        (AppMode::Api, Focus::Processes) => app.select_next_request(),
        (AppMode::Api, Focus::RequestBody) => app.scroll_request_body_down(1),
        (AppMode::Api, Focus::ResponseBody) | (_, Focus::Output) => app.scroll_log_down(1),
        _ => {}
    }
}

pub(crate) fn handle_ctrl_p(app: &mut App) {
    match (app.app_mode, app.focus) {
        (AppMode::Runbook, Focus::Commands) => app.select_prev_command(),
        (AppMode::Runbook, Focus::Processes) => app.select_prev_process(),
        (AppMode::Api, Focus::Commands) => app.select_prev_api(),
        (AppMode::Api, Focus::Variables) => app.select_prev_variable(),
        (AppMode::Api, Focus::Processes) => app.select_prev_request(),
        (AppMode::Api, Focus::RequestBody) => app.scroll_request_body_up(1),
        (AppMode::Api, Focus::ResponseBody) | (_, Focus::Output) => app.scroll_log_up(1),
        _ => {}
    }
}

pub(crate) fn handle_commands_key(app: &mut App, key: KeyEvent, tx: UnboundedSender<AppEvent>) {
    match key.code {
        KeyCode::Esc => {
            if app.input_mode == InputMode::Search {
                app.input_mode = InputMode::Normal;
            }
        }
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
        }
        KeyCode::Up => app.select_prev_command(),
        KeyCode::Down => app.select_next_command(),
        KeyCode::Enter => app.run_selected(tx),
        _ => {
            if app.input_mode == InputMode::Search {
                match key.code {
                    KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                        app.search_push(c);
                    }
                    KeyCode::Backspace => app.search_pop(),
                    _ => {}
                }
            } else {
                app.try_keybinding(&key, tx);
            }
        }
    }
}

pub(crate) fn handle_apis_key(app: &mut App, key: KeyEvent, tx: UnboundedSender<AppEvent>) {
    match key.code {
        KeyCode::Esc => {
            if app.input_mode == InputMode::Search {
                app.input_mode = InputMode::Normal;
            }
        }
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
        }
        KeyCode::Up => app.select_prev_api(),
        KeyCode::Down => app.select_next_api(),
        KeyCode::Enter => app.run_selected(tx),
        _ => {
            if app.input_mode == InputMode::Search {
                match key.code {
                    KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                        app.search_push(c);
                    }
                    KeyCode::Backspace => app.search_pop(),
                    _ => {}
                }
            } else {
                app.try_keybinding(&key, tx);
            }
        }
    }
}

pub(crate) fn handle_processes_key(app: &mut App, key: KeyEvent) {
    let page = app.log_area_height.max(1) as usize;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.focus = Focus::Commands,
        KeyCode::Up => app.select_prev_process(),
        KeyCode::Down => app.select_next_process(),
        KeyCode::PageUp => app.scroll_log_up(page),
        KeyCode::PageDown => app.scroll_log_down(page),
        KeyCode::Home => app.scroll_log_top(),
        KeyCode::End => app.scroll_log_bottom(),
        _ => {}
    }
}

pub(crate) fn handle_requests_key(app: &mut App, key: KeyEvent) {
    let page = app.log_area_height.max(1) as usize;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.focus = Focus::Commands,
        KeyCode::Up => app.select_prev_request(),
        KeyCode::Down => app.select_next_request(),
        KeyCode::PageUp => app.scroll_log_up(page),
        KeyCode::PageDown => app.scroll_log_down(page),
        KeyCode::Home => app.scroll_log_top(),
        KeyCode::End => app.scroll_log_bottom(),
        _ => {}
    }
}

pub(crate) fn handle_output_key(app: &mut App, key: KeyEvent) {
    let page = app.log_area_height.max(1) as usize;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.focus = Focus::Processes,
        KeyCode::Up => app.scroll_log_up(1),
        KeyCode::Down => app.scroll_log_down(1),
        KeyCode::PageUp => app.scroll_log_up(page),
        KeyCode::PageDown => app.scroll_log_down(page),
        KeyCode::Home => app.scroll_log_top(),
        KeyCode::End => app.scroll_log_bottom(),
        _ => {}
    }
}

pub(crate) fn handle_import_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.input_mode = InputMode::Normal,
        KeyCode::Enter => app.import_enter_selected(),
        KeyCode::Up => app.select_prev_import_entry(),
        KeyCode::Down => app.select_next_import_entry(),
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.select_next_import_entry();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.select_prev_import_entry();
        }
        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            app.import_filter_push(c);
        }
        KeyCode::Backspace => app.import_filter_pop(),
        _ => {}
    }
}

pub(crate) fn handle_request_body_key(app: &mut App, key: KeyEvent) {
    let page = app.request_body_area_height.max(1) as usize;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.focus = Focus::Processes,
        KeyCode::Up => app.scroll_request_body_up(1),
        KeyCode::Down => app.scroll_request_body_down(1),
        KeyCode::PageUp => app.scroll_request_body_up(page),
        KeyCode::PageDown => app.scroll_request_body_down(page),
        KeyCode::Home => app.scroll_request_body_top(),
        KeyCode::End => app.scroll_request_body_bottom(),
        _ => {}
    }
}
