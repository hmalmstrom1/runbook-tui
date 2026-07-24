pub mod api;
pub mod app;
pub mod config;
pub mod env;
pub mod keybinding;
pub mod process;
pub mod theme;
pub mod ui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::api::parse_collection;
use crate::app::{read_input, App, AppEvent, handle_event};
use crate::config::read_config;
use crate::env::parse_variable_groups;
use crate::ui::{install_panic_hook, TerminalGuard, ui};

pub async fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (mut path, mut force_api) = (PathBuf::from("runbook.toml"), false);
    let (mut env_path, mut env_name) = (None, None);

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--api" => {
                force_api = true;
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("--api requires a collection JSON file path");
                }
                path = PathBuf::from(&args[i]);
            }
            "--env" => {
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("--env requires a variable group JSON file path");
                }
                env_path = Some(PathBuf::from(&args[i]));
            }
            "--environment" => {
                i += 1;
                if i >= args.len() {
                    anyhow::bail!("--environment requires an environment name");
                }
                env_name = Some(args[i].clone());
            }
            _ if i == args.len() - 1 => {
                path = PathBuf::from(&args[i]);
            }
            _ => {
                anyhow::bail!("Unknown argument: {}", args[i]);
            }
        }
        i += 1;
    }

    if !path.exists() {
        anyhow::bail!(
            "Config not found: {}\nCreate a TOML file with [[commands]] entries or pass a Postman collection JSON file.",
            path.display()
        );
    }

    if let Some(ref p) = env_path && !p.exists() {
        anyhow::bail!("Variable group file not found: {}", p.display());
    }

    install_panic_hook();

    let client = reqwest::Client::new();
    let mut guard = TerminalGuard::setup()?;
    let terminal = guard.terminal();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut app = if force_api || path.extension().and_then(|e| e.to_str()) == Some("json") {
        let parsed = parse_collection(&path)?;
        let groups = env_path.as_deref().map(parse_variable_groups).transpose()?.unwrap_or_default();
        let selected = env_name.and_then(|name| groups.iter().position(|(n, _)| n == &name));
        App::new_api(parsed.apis, client, parsed.variables, groups, selected)
    } else {
        let commands = read_config(&path)?;
        App::new(commands, client)
    };

    let running = Arc::new(AtomicBool::new(true));
    let running_input = running.clone();
    let input_tx = tx.clone();
    let input_handle = tokio::task::spawn_blocking(move || read_input(running_input, input_tx));

    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let result = loop {
        terminal.draw(|f| ui(&mut app, f))?;
        let event = tokio::select! {
            _ = interval.tick() => AppEvent::Tick,
            Some(e) = rx.recv() => e,
        };
        handle_event(&mut app, event, tx.clone());
        if app.quit {
            break Ok(());
        }
    };

    running.store(false, Ordering::Relaxed);
    let _ = input_handle.await;
    result
}
