pub mod api;
pub mod app;
pub mod config;
pub mod env;
pub mod keybinding;
pub mod process;
pub mod theme;
pub mod ui;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

use rust_i18n::t;

use crate::api::parse_collection;
use crate::app::{read_input, App, AppEvent, handle_event};
use crate::config::{load_global_editor, read_config, resolve_editor};
use crate::env::parse_variable_groups;
use crate::ui::{install_panic_hook, TerminalGuard, ui};

rust_i18n::i18n!("locales", fallback = "en");

pub async fn run() -> Result<()> {
    if let Ok(locale) = std::env::var("RUST_I18N_LOCALE") {
        if !locale.is_empty() {
            rust_i18n::set_locale(&locale);
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let mut path_args: Vec<(PathBuf, bool)> = Vec::new();
    let (mut env_path, mut env_name) = (None, None);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "-V" => {
                println!("{}", t!("app.version", version = env!("CARGO_PKG_VERSION")));
                return Ok(());
            }
            "--api" => {
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("{}", t!("lib.api_arg_required"));
                }
                path_args.push((PathBuf::from(&args[i]), true));
            }
            "--env" => {
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("{}", t!("lib.env_arg_required"));
                }
                env_path = Some(PathBuf::from(&args[i]));
            }
            "--environment" => {
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("{}", t!("lib.environment_arg_required"));
                }
                env_name = Some(args[i].clone());
            }
            _ => {
                path_args.push((PathBuf::from(&args[i]), false));
            }
        }
        i += 1;
    }

    if path_args.is_empty() {
        path_args.push((PathBuf::from("runbook.toml"), false));
    }

    for (path, _) in &path_args {
        if !path.exists() {
            anyhow::bail!(
                "{}",
                t!("lib.config_not_found", path = path.display().to_string())
            );
        }
    }

    if let Some(ref p) = env_path && !p.exists() {
        anyhow::bail!("{}", t!("lib.variable_group_not_found", path = p.display().to_string()));
    }

    install_panic_hook();

    let global_editor = load_global_editor();
    let client = reqwest::Client::new();
    let mut guard = TerminalGuard::setup()?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut tabs: Vec<App> = Vec::new();
    for (path, force_api) in &path_args {
        let app = load_app(
            path,
            client.clone(),
            env_path.as_deref(),
            env_name.as_deref(),
            *force_api,
            global_editor.clone(),
        )?;
        tabs.push(app);
    }
    for (i, tab) in tabs.iter_mut().enumerate() {
        tab.tab_idx = i;
    }

    let mut current_tab = 0usize;

    let running = Arc::new(AtomicBool::new(true));
    let input_paused = Arc::new(AtomicBool::new(false));
    let running_input = running.clone();
    let paused_input = input_paused.clone();
    let input_tx = tx.clone();
    let input_handle = tokio::task::spawn_blocking(move || read_input(running_input, paused_input, input_tx));

    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let result = loop {
        let titles: Vec<String> = tabs.iter().map(|t| t.tab_title.clone()).collect();
        for tab in &mut tabs {
            tab.tab_titles = titles.clone();
        }

        guard.terminal().draw(|f| ui(&mut tabs[current_tab], f))?;
        let event = tokio::select! {
            _ = interval.tick() => AppEvent::Tick,
            Some(e) = rx.recv() => e,
        };
        match event {
            AppEvent::Tick => {
                for tab in &mut tabs {
                    handle_event(tab, AppEvent::Tick, tx.clone());
                }
            }
            AppEvent::Input(evt) => {
                let editor_opened = handle_input(evt, &mut tabs, &mut current_tab, &tx, &mut guard, &input_paused)?;
                if editor_opened {
                    drain_input_events(&mut rx, &tx);
                }
            }
            AppEvent::ProcessLine { tab, .. }
            | AppEvent::ProcessExit { tab, .. }
            | AppEvent::ProcessError { tab, .. }
            | AppEvent::ApiResponse { tab, .. }
            | AppEvent::ApiError { tab, .. } => {
                if let Some(t) = tabs.get_mut(tab) {
                    handle_event(t, event, tx.clone());
                }
            }
            AppEvent::SwitchTab { tab } => {
                if tab < tabs.len() {
                    current_tab = tab;
                }
            }
            AppEvent::NewTabImport { path } => {
                match load_app(
                    &path,
                    client.clone(),
                    env_path.as_deref(),
                    env_name.as_deref(),
                    false,
                    global_editor.clone(),
                ) {
                    Ok(mut app) => {
                        app.tab_idx = tabs.len();
                        tabs.push(app);
                        current_tab = tabs.len() - 1;
                    }
                    Err(e) => {
                        tabs[current_tab].set_message(t!("lib.load_tab_error", error = e).to_string());
                    }
                }
            }
        }
        if tabs.iter().any(|t| t.quit) {
            break Ok(());
        }
    };

    running.store(false, Ordering::Relaxed);
    let _ = input_handle.await;
    result
}

fn load_app(
    path: &Path,
    client: reqwest::Client,
    env_path: Option<&Path>,
    env_name: Option<&str>,
    force_api: bool,
    global_editor: Option<String>,
) -> Result<App> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let is_api_file = force_api || ext == "json" || ext == "yaml" || ext == "yml";

    let mut app = if is_api_file {
        let parsed = parse_collection(path)?;
        let env_groups = env_path.map(parse_variable_groups).transpose()?.unwrap_or_default();
        let mut secret_keys = parsed.secret_keys;
        secret_keys.extend(env_groups.secret_keys);
        let selected = env_name.and_then(|name| env_groups.groups.iter().position(|(n, _)| n == name));
        App::new_api(parsed.apis, client, parsed.variables, secret_keys, env_groups.groups, selected)
    } else {
        let cfg = read_config(path)?;
        let mut app = App::new(cfg.commands, client);
        app.editor = resolve_editor(cfg.editor, global_editor.clone());
        app
    };

    app.tab_path = path.to_path_buf();
    if app.editor.is_empty() {
        app.editor = resolve_editor(None, global_editor);
    }
    app.tab_title = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
    Ok(app)
}

fn handle_input(
    evt: Event,
    tabs: &mut [App],
    current_tab: &mut usize,
    tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
    guard: &mut TerminalGuard,
    input_paused: &Arc<AtomicBool>,
) -> Result<bool> {
    if let Event::Key(key) = evt {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }
        if key.code == KeyCode::F(2) {
            if tabs.len() < 2 {
                return Ok(false);
            }
            if tabs.len() == 2 {
                *current_tab = 1 - *current_tab;
            } else {
                let titles = tabs.iter().map(|t| t.tab_title.clone()).collect();
                tabs[*current_tab].open_tab_select(titles);
            }
            return Ok(false);
        }
        if key.code == KeyCode::F(4) {
            let path = tabs[*current_tab].tab_path.clone();
            let editor = tabs[*current_tab].editor.clone();
            input_paused.store(true, Ordering::Relaxed);
            let result = guard.run_editor(&path, &editor);
            input_paused.store(false, Ordering::Relaxed);
            if let Err(e) = result {
                tabs[*current_tab].set_message(t!("lib.editor_failed", error = e).to_string());
            }
            return Ok(true);
        }
        if key.code == KeyCode::Right && key.modifiers.contains(KeyModifiers::CONTROL) {
            if tabs.len() > 1 {
                *current_tab = (*current_tab + 1) % tabs.len();
            }
            return Ok(false);
        }
        if key.code == KeyCode::Left && key.modifiers.contains(KeyModifiers::CONTROL) {
            if tabs.len() > 1 {
                *current_tab = (*current_tab + tabs.len() - 1) % tabs.len();
            }
            return Ok(false);
        }
        handle_event(&mut tabs[*current_tab], AppEvent::Input(Event::Key(key)), tx.clone());
    } else {
        handle_event(&mut tabs[*current_tab], AppEvent::Input(evt), tx.clone());
    }
    Ok(false)
}

fn drain_input_events(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>,
) {
    while let Ok(evt) = rx.try_recv() {
        if matches!(evt, AppEvent::Input(_)) {
            continue;
        }
        let _ = tx.send(evt);
    }
}
