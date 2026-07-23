use ratatui::style::{Color, Modifier, Style};

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
