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
    commands: Vec<RawCommand>,
}

pub(crate) fn read_config(path: &Path) -> Result<Vec<Command>> {
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
    Ok(commands)
}
