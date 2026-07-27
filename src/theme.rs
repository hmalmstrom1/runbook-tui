use std::fs;
use std::path::PathBuf;

use ratatui::style::{Color, Modifier, Style};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) border: Style,
    pub(crate) focused: Style,
    pub(crate) highlight: Style,
    pub(crate) key: Style,
    pub(crate) running: Style,
    pub(crate) success: Style,
    pub(crate) error: Style,
    pub(crate) dim: Style,
    pub(crate) input: Style,
    pub(crate) json_string: Style,
    pub(crate) json_number: Style,
    pub(crate) json_bool: Style,
    pub(crate) json_punctuation: Style,
    pub(crate) xml_tag: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            border: Style::default(),
            focused: Style::default().fg(Color::Cyan),
            highlight: Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD),
            key: Style::default().fg(Color::Yellow),
            running: Style::default().fg(Color::Yellow),
            success: Style::default().fg(Color::Green),
            error: Style::default().fg(Color::Red),
            dim: Style::default().fg(Color::DarkGray),
            input: Style::default().fg(Color::Yellow),
            json_string: Style::default().fg(Color::Green),
            json_number: Style::default().fg(Color::Yellow),
            json_bool: Style::default().fg(Color::Magenta),
            json_punctuation: Style::default().fg(Color::White),
            xml_tag: Style::default().fg(Color::Cyan),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedTheme {
    name: String,
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("runbook-tui"))
}

pub(crate) fn load_saved_theme_name() -> Option<String> {
    let path = config_dir()?.join("theme.toml");
    let content = fs::read_to_string(path).ok()?;
    let saved: SavedTheme = toml::from_str(&content).ok()?;
    Some(saved.name)
}

pub(crate) fn save_theme_name(name: &str) -> std::io::Result<()> {
    let dir = config_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, t!("theme.no_config_directory").to_string())
    })?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("theme.toml");
    let saved = SavedTheme { name: name.to_string() };
    let content = toml::to_string(&saved)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, content)
}

fn hex(s: &str) -> Color {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return Color::Reset;
    }
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(0);
    Color::Rgb(r, g, b)
}

#[allow(clippy::too_many_arguments)]
fn make_theme(
    border: Color,
    focused: Color,
    highlight_bg: Color,
    highlight_fg: Color,
    key: Color,
    running: Color,
    success: Color,
    error: Color,
    dim: Color,
    input: Color,
    json_string: Color,
    json_number: Color,
    json_bool: Color,
    json_punctuation: Color,
    xml_tag: Color,
) -> Theme {
    Theme {
        border: Style::default().fg(border),
        focused: Style::default().fg(focused),
        highlight: Style::default()
            .bg(highlight_bg)
            .fg(highlight_fg)
            .add_modifier(Modifier::BOLD),
        key: Style::default().fg(key),
        running: Style::default().fg(running),
        success: Style::default().fg(success),
        error: Style::default().fg(error),
        dim: Style::default().fg(dim),
        input: Style::default().fg(input),
        json_string: Style::default().fg(json_string),
        json_number: Style::default().fg(json_number),
        json_bool: Style::default().fg(json_bool),
        json_punctuation: Style::default().fg(json_punctuation),
        xml_tag: Style::default().fg(xml_tag),
    }
}

pub(crate) fn theme_names() -> &'static [&'static str] {
    &[
        "Default",
        "Catppuccin Mocha",
        "Catppuccin Latte",
        "Base16 Default Dark",
        "Base16 Default Light",
        "Base16 Ocean Dark",
        "Base16 Ocean Light",
        "Base16 Monokai",
        "Base16 One Dark",
        "Base16 One Light",
    ]
}

pub(crate) fn theme_by_name(name: &str) -> Theme {
    match name {
        "Catppuccin Mocha" => catppuccin_mocha(),
        "Catppuccin Latte" => catppuccin_latte(),
        "Base16 Default Dark" => base16_default_dark(),
        "Base16 Default Light" => base16_default_light(),
        "Base16 Ocean Dark" => base16_ocean_dark(),
        "Base16 Ocean Light" => base16_ocean_light(),
        "Base16 Monokai" => base16_monokai(),
        "Base16 One Dark" => base16_one_dark(),
        "Base16 One Light" => base16_one_light(),
        _ => Theme::default(),
    }
}

fn catppuccin_mocha() -> Theme {
    make_theme(
        hex("#7f849c"),
        hex("#89b4fa"),
        hex("#45475a"),
        hex("#cdd6f4"),
        hex("#f9e2af"),
        hex("#f9e2af"),
        hex("#a6e3a1"),
        hex("#f38ba8"),
        hex("#6c7086"),
        hex("#f9e2af"),
        hex("#a6e3a1"),
        hex("#f9e2af"),
        hex("#cba6f7"),
        hex("#cdd6f4"),
        hex("#94e2d5"),
    )
}

fn catppuccin_latte() -> Theme {
    make_theme(
        hex("#8c8fa1"),
        hex("#1e66f5"),
        hex("#bcc0cc"),
        hex("#4c4f69"),
        hex("#df8e1d"),
        hex("#df8e1d"),
        hex("#40a02b"),
        hex("#d20f39"),
        hex("#9ca0b0"),
        hex("#df8e1d"),
        hex("#40a02b"),
        hex("#df8e1d"),
        hex("#8839ef"),
        hex("#4c4f69"),
        hex("#179299"),
    )
}

fn base16_theme(colors: [&str; 16]) -> Theme {
    make_theme(
        hex(colors[4]),
        hex(colors[13]),
        hex(colors[2]),
        hex(colors[7]),
        hex(colors[10]),
        hex(colors[10]),
        hex(colors[11]),
        hex(colors[8]),
        hex(colors[3]),
        hex(colors[10]),
        hex(colors[11]),
        hex(colors[10]),
        hex(colors[14]),
        hex(colors[5]),
        hex(colors[12]),
    )
}

fn base16_default_dark() -> Theme {
    base16_theme([
        "#181818", "#282828", "#383838", "#585858",
        "#b8b8b8", "#d8d8d8", "#e8e8e8", "#f8f8f8",
        "#ab4642", "#dc9656", "#f7ca88", "#a1b56c",
        "#86c1b9", "#7cafc2", "#ba8baf", "#a16946",
    ])
}

fn base16_default_light() -> Theme {
    base16_theme([
        "#f8f8f8", "#e8e8e8", "#d8d8d8", "#b8b8b8",
        "#585858", "#383838", "#282828", "#181818",
        "#ab4642", "#dc9656", "#f7ca88", "#a1b56c",
        "#86c1b9", "#7cafc2", "#ba8baf", "#a16946",
    ])
}

fn base16_ocean_dark() -> Theme {
    base16_theme([
        "#2b303b", "#343d46", "#4f5b66", "#65737e",
        "#a7adba", "#c0c5ce", "#dfe1e8", "#eff1f5",
        "#bf616a", "#d08770", "#ebcb8b", "#a3be8c",
        "#96b5b4", "#8fa1b3", "#b48ead", "#ab7967",
    ])
}

fn base16_ocean_light() -> Theme {
    base16_theme([
        "#eff1f5", "#dfe1e8", "#c0c5ce", "#a7adba",
        "#65737e", "#4f5b66", "#343d46", "#2b303b",
        "#bf616a", "#d08770", "#ebcb8b", "#a3be8c",
        "#96b5b4", "#8fa1b3", "#b48ead", "#ab7967",
    ])
}

fn base16_monokai() -> Theme {
    base16_theme([
        "#272822", "#383830", "#49483e", "#75715e",
        "#a59f85", "#f8f8f2", "#f5f4f1", "#f9f8f5",
        "#f92672", "#fd971f", "#f4bf75", "#a6e22e",
        "#a1efe4", "#66d9ef", "#ae81ff", "#cc6633",
    ])
}

fn base16_one_dark() -> Theme {
    base16_theme([
        "#1e222a", "#353b45", "#3e4451", "#545862",
        "#565c64", "#abb2bf", "#b6bdca", "#c8ccd4",
        "#e06c75", "#d19a66", "#e5c07b", "#98c379",
        "#56b6c2", "#61afef", "#c678dd", "#be5046",
    ])
}

fn base16_one_light() -> Theme {
    base16_theme([
        "#fafafa", "#f0f0f1", "#e5e5e6", "#a0a1a7",
        "#696c77", "#383a42", "#202227", "#090a0f",
        "#ca1243", "#d75f00", "#c18401", "#50a14f",
        "#0184bc", "#4078f2", "#a626a4", "#986801",
    ])
}
