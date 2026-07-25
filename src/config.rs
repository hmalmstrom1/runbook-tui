use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::keybinding::KeyBinding;

#[derive(Debug, Clone)]
pub(crate) struct Command {
    pub(crate) title: String,
    pub(crate) key: KeyBinding,
    pub(crate) command: String,
}

#[derive(Debug, Deserialize)]
struct RawCommand {
    title: String,
    keybinding: String,
    command: String,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    commands: Vec<RawCommand>,
    editor: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RunbookConfig {
    pub(crate) commands: Vec<Command>,
    pub(crate) editor: Option<String>,
}

pub(crate) fn read_config(path: &Path) -> Result<RunbookConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config {}", path.display()))?;
    let config: ConfigFile = toml::from_str(&content)
        .with_context(|| "Failed to parse config as TOML")?;
    let mut commands = Vec::new();
    for raw in config.commands {
        let key = KeyBinding::parse(&raw.keybinding)
            .with_context(|| format!("Invalid keybinding '{}' for command '{}'", raw.keybinding, raw.title))?;
        commands.push(Command {
            title: raw.title,
            key,
            command: raw.command,
        });
    }
    Ok(RunbookConfig {
        commands,
        editor: config.editor.filter(|s| !s.is_empty()),
    })
}

pub(crate) fn load_global_editor() -> Option<String> {
    let path = dirs::config_dir()?.join("runbook-tui").join("config.toml");
    let content = fs::read_to_string(path).ok()?;
    let config: ConfigFile = toml::from_str(&content).ok()?;
    config.editor.filter(|s| !s.is_empty())
}

pub(crate) fn resolve_editor(file_editor: Option<String>, global_editor: Option<String>) -> String {
    file_editor
        .filter(|s| !s.is_empty())
        .or(global_editor.filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("VISUAL").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "vi".to_string())
}
