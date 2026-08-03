use std::collections::{BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use rust_i18n::t;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{self, ApiItem, ApiRequest, ApiStatus, format_body, run_api_request};
use crate::config::{load_global_editor, read_config, resolve_editor, Command};
use crate::cwl;
use crate::keybinding::KeyBinding;
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
            ProcessStatus::Running => t!("process.status.running").to_string(),
            ProcessStatus::Exited(c) => t!("process.status.exit", code = c.to_string()).to_string(),
            ProcessStatus::Failed(e) => t!("process.status.failed", error = e).to_string(),
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
    EnvironmentSelect,
    TabSelect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnvironmentChoice {
    Collection,
    Environment(usize),
    ToggleEnvOverlay,
    ImportEnvGroup,
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
    Cwl,
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
    pub(crate) theme_name: String,
    pub(crate) collection_variables: Vec<(String, String)>,
    pub(crate) secret_keys: BTreeSet<String>,
    pub(crate) environments: Vec<(String, Vec<(String, String)>)>,
    pub(crate) selected_environment: Option<usize>,
    pub(crate) env_overlay: bool,
    pub(crate) environment_state: ListState,
    pub(crate) variables: Vec<(String, String)>,
    pub(crate) variable_state: ListState,
    pub(crate) variable_edit_index: Option<usize>,
    pub(crate) variable_edit_value: String,
    pub(crate) variable_edit_reveal: bool,
    pub(crate) zoom: Option<Focus>,
    pub(crate) message: String,
    pub(crate) message_until: Option<Instant>,
    pub(crate) importing_env_group: bool,
    pub(crate) importing_new_tab: bool,
    pub(crate) tab_idx: usize,
    pub(crate) tab_title: String,
    pub(crate) tab_titles: Vec<String>,
    pub(crate) tab_select_state: ListState,
    pub(crate) tab_path: PathBuf,
    pub(crate) editor: String,
    pub(crate) cwl_doc: Option<cwl_core::documents::CWLDocument>,
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
            theme_name: "Default".to_string(),
            collection_variables: Vec::new(),
            secret_keys: BTreeSet::new(),
            environments: Vec::new(),
            selected_environment: None,
            env_overlay: false,
            environment_state: ListState::default(),
            variables: Vec::new(),
            variable_state: ListState::default(),
            variable_edit_index: None,
            variable_edit_value: String::new(),
            variable_edit_reveal: false,
            zoom: None,
            message: String::new(),
            message_until: None,
            importing_env_group: false,
            importing_new_tab: false,
            tab_idx: 0,
            tab_title: String::new(),
            tab_titles: Vec::new(),
            tab_select_state: ListState::default(),
            tab_path: PathBuf::new(),
            editor: String::new(),
            cwl_doc: None,
        };
        if let Some(name) = crate::theme::load_saved_theme_name() {
            app.theme = crate::theme::theme_by_name(&name);
            app.theme_name = name;
        }
        app.update_filtered();
        app
    }

    pub(crate) fn new_api(
        apis: Vec<ApiItem>,
        client: reqwest::Client,
        variables: Vec<(String, String)>,
        secret_keys: BTreeSet<String>,
        environments: Vec<(String, Vec<(String, String)>)>,
        selected_environment: Option<usize>,
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
            theme_name: "Default".to_string(),
            collection_variables: variables,
            secret_keys,
            environments,
            selected_environment: None,
            env_overlay: false,
            environment_state: ListState::default(),
            variables: Vec::new(),
            variable_state: ListState::default(),
            variable_edit_index: None,
            variable_edit_value: String::new(),
            variable_edit_reveal: false,
            zoom: None,
            message: String::new(),
            message_until: None,
            importing_env_group: false,
            importing_new_tab: false,
            tab_idx: 0,
            tab_title: String::new(),
            tab_titles: Vec::new(),
            tab_select_state: ListState::default(),
            tab_path: PathBuf::new(),
            editor: String::new(),
            cwl_doc: None,
        };
        if let Some(name) = crate::theme::load_saved_theme_name() {
            app.theme = crate::theme::theme_by_name(&name);
            app.theme_name = name;
        }
        app.set_environment(selected_environment);
        app.update_filtered();
        app.variable_state.select(Some(0));
        app
    }

    pub(crate) fn new_cwl(
        path: &std::path::Path,
        client: reqwest::Client,
    ) -> anyhow::Result<Self> {
        let doc = cwl::load_cwl(path)?;
        let title = cwl::cwl_title(&doc);
        let run_cmd = Command {
            title: format!("Run CWL: {}", title),
            key: KeyBinding::parse("r").ok_or_else(|| anyhow::anyhow!("invalid keybinding"))?,
            command: path.display().to_string(),
        };
        let mut app = Self::new(vec![run_cmd], client);
        app.app_mode = AppMode::Cwl;
        app.cwl_doc = Some(doc);
        Ok(app)
    }

    fn recompute_variables(&mut self) {
        let mut merged = self.collection_variables.clone();
        if let Some(idx) = self.selected_environment
            && let Some((_, env_vars)) = self.environments.get(idx)
        {
            for (key, value) in env_vars {
                if let Some(entry) = merged.iter_mut().find(|(k, _)| k == key) {
                    entry.1 = value.clone();
                } else {
                    merged.push((key.clone(), value.clone()));
                }
            }
        }
        if self.env_overlay {
            for (key, value) in crate::env::env_overrides(&self.collection_variables) {
                if let Some(entry) = merged.iter_mut().find(|(k, _)| k == &key) {
                    entry.1 = value;
                } else {
                    merged.push((key, value));
                }
            }
        }
        self.variables = merged;
    }

    pub(crate) fn set_environment(&mut self, index: Option<usize>) {
        if let Some(idx) = index {
            if idx >= self.environments.len() {
                return;
            }
            self.selected_environment = Some(idx);
        } else {
            self.selected_environment = None;
        }
        self.recompute_variables();
        self.variable_edit_index = None;
        self.variable_edit_value.clear();
        self.variable_state.select(Some(0));
    }

    pub(crate) fn environment_label(&self) -> String {
        let base = self
            .selected_environment
            .and_then(|idx| self.environments.get(idx))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| t!("environment.collection").to_string());
        if self.env_overlay {
            t!("environment.plus_env", base = base).to_string()
        } else {
            base
        }
    }

    fn environment_overlay_available(&self) -> bool {
        !crate::env::env_overrides(&self.collection_variables).is_empty()
    }

    fn mask_secret_values(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (key, value) in &self.variables {
            if self.secret_keys.contains(key) {
                result = result.replace(value, "********");
            }
        }
        result
    }

    pub(crate) fn environment_choices(&self) -> Vec<(String, EnvironmentChoice)> {
        let mut choices = vec![(t!("environment.collection").to_string(), EnvironmentChoice::Collection)];
        for (idx, (name, _)) in self.environments.iter().enumerate() {
            choices.push((name.clone(), EnvironmentChoice::Environment(idx)));
        }
        choices.push((t!("environment.import_group").to_string(), EnvironmentChoice::ImportEnvGroup));
        if self.environment_overlay_available() {
            let label = if self.env_overlay {
                t!("environment.overlay_checked")
            } else {
                t!("environment.overlay_unchecked")
            };
            choices.push((label.to_string(), EnvironmentChoice::ToggleEnvOverlay));
        }
        choices
    }

    pub(crate) fn select_next_environment(&mut self) {
        let max = self.environment_choices().len().saturating_sub(1);
        let i = self.environment_state.selected().unwrap_or(0);
        let next = (i + 1).min(max);
        self.environment_state.select(Some(next));
    }

    pub(crate) fn select_prev_environment(&mut self) {
        let i = self.environment_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.environment_state.select(Some(prev));
    }

    pub(crate) fn open_environment_select(&mut self) {
        self.input_mode = InputMode::EnvironmentSelect;
        let choices = self.environment_choices();
        let selected = self
            .selected_environment
            .and_then(|idx| choices.iter().position(|(_, choice)| matches!(choice, EnvironmentChoice::Environment(i) if *i == idx)))
            .unwrap_or(0);
        self.environment_state.select(Some(selected));
    }

    pub(crate) fn confirm_environment_select(&mut self) {
        let choices = self.environment_choices();
        let index = self.environment_state.selected().unwrap_or(0);
        let choice = choices.get(index).map(|(_, choice)| *choice);
        match choice {
            Some(EnvironmentChoice::Collection) => {
                self.set_environment(None);
                self.input_mode = InputMode::Normal;
            }
            Some(EnvironmentChoice::Environment(idx)) => {
                self.set_environment(Some(idx));
                self.input_mode = InputMode::Normal;
            }
            Some(EnvironmentChoice::ImportEnvGroup) => {
                self.start_env_group_import();
            }
            Some(EnvironmentChoice::ToggleEnvOverlay) => {
                self.env_overlay = !self.env_overlay;
                self.recompute_variables();
                self.environment_state.select(Some(index));
                // keep the menu open so the user can select an environment next
            }
            None => {
                self.input_mode = InputMode::Normal;
            }
        }
    }

    pub(crate) fn cancel_environment_select(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    pub(crate) fn update_filtered(&mut self) {
        let q = self.search.to_lowercase();
        match self.app_mode {
            AppMode::Runbook | AppMode::Cwl => {
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
            && let Some((key, value)) = self.variables.get(i)
        {
            self.variable_edit_index = Some(i);
            self.variable_edit_value = value.clone();
            self.variable_edit_reveal = !self.secret_keys.contains(key);
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

    pub(crate) fn toggle_variable_edit_reveal(&mut self) {
        self.variable_edit_reveal = !self.variable_edit_reveal;
    }

    pub(crate) fn toggle_zoom(&mut self) {
        self.zoom = if self.zoom == Some(self.focus) {
            None
        } else {
            Some(self.focus)
        };
    }

    pub(crate) fn cycle_theme(&mut self) {
        let names = crate::theme::theme_names();
        let current = names
            .iter()
            .position(|&n| n == self.theme_name)
            .unwrap_or(0);
        let next = (current + 1) % names.len();
        self.theme_name = names[next].to_string();
        self.theme = crate::theme::theme_by_name(names[next]);
        match crate::theme::save_theme_name(&self.theme_name) {
            Ok(()) => self.set_message(t!("message.theme_changed", name = self.theme_name.clone()).to_string()),
            Err(e) => self.set_message(t!("message.theme_save_failed", error = e).to_string()),
        }
    }

    pub(crate) fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.message_until = Some(Instant::now() + Duration::from_secs(5));
    }

    pub(crate) fn clear_message(&mut self) {
        self.message.clear();
        self.message_until = None;
    }

    fn export_timestamp(&self) -> u128 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    pub(crate) fn export_output(&mut self) {
        let result = match self.app_mode {
            AppMode::Runbook | AppMode::Cwl => self.export_runbook_output(),
            AppMode::Api => self.export_api_output(),
        };
        match result {
            Ok(path) => self.set_message(t!("message.exported", path = path.display().to_string()).to_string()),
            Err(e) => self.set_message(t!("message.export_failed", error = e).to_string()),
        }
    }

    fn export_runbook_output(&mut self) -> std::io::Result<PathBuf> {
        let process = self
            .selected_process()
            .ok_or_else(|| std::io::Error::other(t!("export.no_process_selected").to_string()))?;
        let timestamp = self.export_timestamp();
        let filename = format!("rbt-export-runbook-{}.txt", timestamp);
        let path = env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(&filename);

        let output = process.output.iter().cloned().collect::<Vec<_>>().join("\n");
        let content = t!(
            "export.runbook_template",
            command = process._command.clone(),
            output = output
        ).to_string();
        fs::write(&path, content)?;
        Ok(path)
    }

    fn export_api_output(&mut self) -> std::io::Result<PathBuf> {
        let request = self
            .selected_request()
            .ok_or_else(|| std::io::Error::other(t!("export.no_request_selected").to_string()))?;
        let timestamp = self.export_timestamp();
        let filename = format!("rbt-export-api-{}.txt", timestamp);
        let path = env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(&filename);

        let request_body = request.request_body.iter().cloned().collect::<Vec<_>>().join("\n");
        let response_body = request.body.iter().cloned().collect::<Vec<_>>().join("\n");
        let status_line = match &request.status {
            ApiStatus::Done(_) => String::new(),
            ApiStatus::Running => t!("api.status.running").to_string(),
            ApiStatus::Failed(e) => t!("api.status.failed", error = e).to_string(),
        };
        let response_headers = request.headers.clone();

        let content = if status_line.is_empty() {
            t!(
                "export.api_template",
                method = request.method.clone(),
                url = request.url.clone(),
                request_headers = request.request_headers.clone(),
                request_body = request_body,
                response_headers = response_headers,
                response_body = response_body
            ).to_string()
        } else {
            t!(
                "export.api_template_with_status",
                method = request.method.clone(),
                url = request.url.clone(),
                request_headers = request.request_headers.clone(),
                request_body = request_body,
                status_line = status_line,
                response_headers = response_headers,
                response_body = response_body
            ).to_string()
        };
        fs::write(&path, content)?;
        Ok(path)
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
        self.importing_env_group = false;
        self.import_path = PathBuf::new();
        self.import_filter.clear();
        self.import_cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.import_message = t!("import.type_to_filter").to_string();
        self.import_error = false;
        self.refresh_import_entries();
        self.import_state.select(Some(0));
    }

    pub(crate) fn start_env_group_import(&mut self) {
        self.input_mode = InputMode::Import;
        self.importing_env_group = true;
        self.import_path = PathBuf::new();
        self.import_filter.clear();
        self.import_cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.import_message = t!("import.select_env_group").to_string();
        self.import_error = false;
        self.refresh_import_entries();
        self.import_state.select(Some(0));
    }

    pub(crate) fn start_new_tab_import(&mut self) {
        self.input_mode = InputMode::Import;
        self.importing_new_tab = true;
        self.importing_env_group = false;
        self.import_path = PathBuf::new();
        self.import_filter.clear();
        self.import_cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.import_message = t!("import.select_new_tab").to_string();
        self.import_error = false;
        self.refresh_import_entries();
        self.import_state.select(Some(0));
    }

    pub(crate) fn open_tab_select(&mut self, titles: Vec<String>) {
        self.input_mode = InputMode::TabSelect;
        self.tab_titles = titles;
        self.tab_select_state.select(Some(self.tab_idx));
    }

    pub(crate) fn select_prev_tab(&mut self) {
        if !self.tab_titles.is_empty() {
            let i = self.tab_select_state.selected().unwrap_or(0);
            let prev = i.checked_sub(1).unwrap_or(self.tab_titles.len() - 1);
            self.tab_select_state.select(Some(prev));
        }
    }

    pub(crate) fn select_next_tab(&mut self) {
        if !self.tab_titles.is_empty() {
            let i = self.tab_select_state.selected().unwrap_or(0);
            let next = (i + 1) % self.tab_titles.len();
            self.tab_select_state.select(Some(next));
        }
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
                self.import_message = t!("import.items_count", count = self.import_entries.len().to_string()).to_string();
                self.import_error = false;
            }
            Err(e) => {
                self.import_message = t!("message.error", error = e).to_string();
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

    pub(crate) fn import_enter_selected(&mut self, tx: &UnboundedSender<AppEvent>) {
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
                self.confirm_import(tx);
            }
        }
    }

    pub(crate) fn confirm_import(&mut self, tx: &UnboundedSender<AppEvent>) {
        let path = self.import_path.clone();
        let tab_idx = self.tab_idx;
        if self.importing_env_group {
            match crate::env::parse_variable_groups(&path) {
                Ok(parsed) => {
                    for group in parsed.groups {
                        self.environments.push(group);
                    }
                    self.secret_keys.extend(parsed.secret_keys);
                    self.recompute_variables();
                    self.importing_env_group = false;
                    self.input_mode = InputMode::EnvironmentSelect;
                    self.set_message(t!("message.env_group_imported").to_string());
                }
                Err(e) => {
                    self.import_message = t!("message.error", error = e).to_string();
                    self.import_error = true;
                }
            }
            return;
        }
        if self.importing_new_tab {
            let _ = tx.send(AppEvent::NewTabImport { path });
            self.importing_new_tab = false;
            self.input_mode = InputMode::Normal;
            return;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let global_editor = load_global_editor();
        let result = if ext == "json" || ext == "yaml" || ext == "yml" {
            api::parse_collection(&path).map(|parsed| {
                let client = self.client.clone();
                *self = Self::new_api(parsed.apis, client, parsed.variables, parsed.secret_keys, Vec::new(), None);
                self.editor = resolve_editor(None, global_editor.clone());
            })
        } else {
            read_config(&path).map(|cfg| {
                let client = self.client.clone();
                *self = Self::new(cfg.commands, client);
                self.editor = resolve_editor(cfg.editor, global_editor.clone());
            })
        };
        if result.is_ok() {
            self.tab_idx = tab_idx;
            self.tab_path = path.clone();
            self.tab_title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
            self.input_mode = InputMode::Normal;
        } else if let Err(e) = result {
            self.import_message = t!("message.error", error = e).to_string();
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

    pub(crate) fn cycle_focus(&mut self, forward: bool) {
        self.input_mode = InputMode::Normal;
        self.zoom = None;
        self.focus = match (self.app_mode, self.focus, forward) {
            (AppMode::Runbook, Focus::Commands, true) => Focus::Processes,
            (AppMode::Runbook, Focus::Processes, true) => Focus::Output,
            (AppMode::Runbook, Focus::Output, true) => Focus::Commands,
            (AppMode::Runbook, Focus::Commands, false) => Focus::Output,
            (AppMode::Runbook, Focus::Output, false) => Focus::Processes,
            (AppMode::Runbook, Focus::Processes, false) => Focus::Commands,
            (AppMode::Cwl, Focus::Commands, true) => Focus::Processes,
            (AppMode::Cwl, Focus::Processes, true) => Focus::Output,
            (AppMode::Cwl, Focus::Output, true) => Focus::Commands,
            (AppMode::Cwl, Focus::Commands, false) => Focus::Output,
            (AppMode::Cwl, Focus::Output, false) => Focus::Processes,
            (AppMode::Cwl, Focus::Processes, false) => Focus::Commands,
            (AppMode::Api, Focus::Commands, true) => Focus::Variables,
            (AppMode::Api, Focus::Variables, true) => Focus::Processes,
            (AppMode::Api, Focus::Processes, true) => Focus::RequestBody,
            (AppMode::Api, Focus::RequestBody, true) => Focus::ResponseBody,
            (AppMode::Api, Focus::ResponseBody, true) => Focus::Commands,
            (AppMode::Api, Focus::Commands, false) => Focus::ResponseBody,
            (AppMode::Api, Focus::ResponseBody, false) => Focus::RequestBody,
            (AppMode::Api, Focus::RequestBody, false) => Focus::Processes,
            (AppMode::Api, Focus::Processes, false) => Focus::Variables,
            (AppMode::Api, Focus::Variables, false) => Focus::Commands,
            _ => Focus::Commands,
        };
        if self.focus == Focus::Processes {
            match self.app_mode {
                AppMode::Runbook | AppMode::Cwl => {
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

        if self.app_mode == AppMode::Cwl {
            tokio::spawn(cwl::run_cwl(self.tab_idx, id, self.tab_path.clone(), tx));
        } else {
            tokio::spawn(run_process(self.tab_idx, id, shell, log_path, tx));
        }
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

        let request_headers = self.mask_secret_values(&request_headers);
        let request_body: VecDeque<String> = request_body
            .into_iter()
            .map(|line| self.mask_secret_values(&line))
            .collect();

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

        tokio::spawn(run_api_request(self.tab_idx, id, item, self.client.clone(), tx));
    }

    pub(crate) fn run_selected(&mut self, tx: UnboundedSender<AppEvent>) {
        match self.app_mode {
            AppMode::Runbook | AppMode::Cwl => {
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
            AppMode::Runbook | AppMode::Cwl => {
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
    ProcessLine { tab: usize, id: usize, line: String },
    ProcessExit { tab: usize, id: usize, code: Option<i32> },
    ProcessError { tab: usize, id: usize, error: String },
    ApiResponse { tab: usize, id: usize, status: u16, headers: String, body: String },
    ApiError { tab: usize, id: usize, error: String },
    SwitchTab { tab: usize },
    NewTabImport { path: PathBuf },
}

pub(crate) fn read_input(
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    tx: UnboundedSender<AppEvent>,
) {
    while running.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }
        if event::poll(Duration::from_millis(25)).unwrap_or(false)
            && let Ok(evt) = event::read()
            && tx.send(AppEvent::Input(evt)).is_err()
        {
            break;
        }
    }
}

pub(crate) fn handle_event(app: &mut App, event: AppEvent, tx: UnboundedSender<AppEvent>) {
    match event {
        AppEvent::Tick => {
            if let Some(until) = app.message_until
                && Instant::now() >= until
            {
                app.clear_message();
            }
        }
        AppEvent::Input(Event::Key(key)) => handle_key(app, key, tx),
        AppEvent::Input(_) => {}
        AppEvent::ProcessLine { id, line, .. } => app.push_process_line(id, line),
        AppEvent::ProcessExit { id, code, .. } => app.set_process_exit(id, code),
        AppEvent::ProcessError { id, error, .. } => app.set_process_error(id, error),
        AppEvent::ApiResponse { id, status, headers, body, .. } => {
            app.push_api_response(id, status, headers, body);
        }
        AppEvent::ApiError { id, error, .. } => app.set_api_error(id, error),
        AppEvent::SwitchTab { .. } | AppEvent::NewTabImport { .. } => {}
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
            app.importing_env_group = false;
            app.importing_new_tab = false;
            app.input_mode = InputMode::Normal;
        } else {
            handle_import_key(app, key, tx);
        }
        return;
    }
    if app.input_mode == InputMode::VariableEdit {
        match key.code {
            KeyCode::Enter => app.confirm_variable_edit(),
            KeyCode::Esc => app.cancel_variable_edit(),
            KeyCode::Char('m') if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                app.toggle_variable_edit_reveal();
            }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                app.variable_edit_push(c);
            }
            KeyCode::Backspace => app.variable_edit_pop(),
            _ => {}
        }
        return;
    }
    if app.input_mode == InputMode::EnvironmentSelect {
        match key.code {
            KeyCode::Enter => app.confirm_environment_select(),
            KeyCode::Esc => app.cancel_environment_select(),
            KeyCode::Up => app.select_prev_environment(),
            KeyCode::Down => app.select_next_environment(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => app.select_prev_environment(),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => app.select_next_environment(),
            _ => {}
        }
        return;
    }
    if app.input_mode == InputMode::TabSelect {
        match key.code {
            KeyCode::Enter => {
                if let Some(i) = app.tab_select_state.selected() {
                    let _ = tx.send(AppEvent::SwitchTab { tab: i });
                }
                app.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => app.input_mode = InputMode::Normal,
            KeyCode::Up => app.select_prev_tab(),
            KeyCode::Down => app.select_next_tab(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => app.select_prev_tab(),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => app.select_next_tab(),
            _ => {}
        }
        return;
    }
    if key.code == KeyCode::Char('m')
        && app.input_mode == InputMode::Normal
        && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        app.toggle_zoom();
        return;
    }
    if key.code == KeyCode::BackTab
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
    {
        app.cycle_focus(false);
        return;
    }
    if key.code == KeyCode::Tab {
        app.cycle_focus(true);
        return;
    }
    if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.export_output();
        return;
    }
    if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
        if app.app_mode == AppMode::Api {
            app.open_environment_select();
        }
        return;
    }
    if key.code == KeyCode::F(3) {
        app.start_new_tab_import();
        return;
    }
    if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.start_import();
        return;
    }
    if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.cycle_theme();
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
        (AppMode::Cwl, Focus::Commands) => handle_commands_key(app, key, tx),
        (AppMode::Cwl, Focus::Processes) => handle_processes_key(app, key),
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
        (AppMode::Cwl, Focus::Commands) => app.select_next_command(),
        (AppMode::Cwl, Focus::Processes) => app.select_next_process(),
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
        (AppMode::Cwl, Focus::Commands) => app.select_prev_command(),
        (AppMode::Cwl, Focus::Processes) => app.select_prev_process(),
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

pub(crate) fn handle_import_key(app: &mut App, key: KeyEvent, tx: UnboundedSender<AppEvent>) {
    match key.code {
        KeyCode::Esc => app.input_mode = InputMode::Normal,
        KeyCode::Enter => app.import_enter_selected(&tx),
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
