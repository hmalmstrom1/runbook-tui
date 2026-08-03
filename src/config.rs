use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use rust_i18n::t;
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
        .with_context(|| t!("config.read_error", path = path.display().to_string()).to_string())?;
    let config: ConfigFile = toml::from_str(&content)
        .with_context(|| t!("config.parse_toml_error").to_string())?;
    let mut commands = Vec::new();
    for raw in config.commands {
        let key = KeyBinding::parse(&raw.keybinding)
            .with_context(|| t!("config.invalid_keybinding", raw = raw.keybinding, title = raw.title).to_string())?;
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
        .or_else(|| pick_editor(&["vim", "vi", "emacs"]))
        .unwrap_or_else(|| "vi".to_string())
}

fn pick_editor(candidates: &[&str]) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    for editor in candidates {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(editor);
            if fs::metadata(&p).ok().filter(|m| m.is_file()).is_some() {
                let path = p
                    .to_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| p.to_string_lossy().into_owned());
                return shlex::try_quote(&path)
                    .ok()
                    .map(|q| q.into_owned())
                    .or(Some(path));
            }
        }
    }
    None
}
