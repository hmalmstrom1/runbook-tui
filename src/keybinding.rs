use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone)]
pub(crate) enum KeyBinding {
    Char(char),
    F(u8),
    Ctrl(char),
}

impl KeyBinding {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let lower = s.to_lowercase();
        if let Some(rest) = lower.strip_prefix("ctrl+") {
            return rest.chars().next().map(KeyBinding::Ctrl);
        }
        if let Some(rest) = lower.strip_prefix('^') {
            return rest.chars().next().map(KeyBinding::Ctrl);
        }
        if let Some(num) = lower.strip_prefix('f')
            && let Ok(n) = num.parse::<u8>()
            && (1..=12).contains(&n)
        {
            return Some(KeyBinding::F(n));
        }
        if s.chars().count() == 1 {
            return s.chars().next().map(KeyBinding::Char);
        }
        None
    }

    pub(crate) fn display(&self) -> String {
        match self {
            KeyBinding::Char(c) => c.to_string(),
            KeyBinding::F(n) => format!("F{}", n),
            KeyBinding::Ctrl(c) => format!("Ctrl+{}", c),
        }
    }

    pub(crate) fn matches(&self, key: &KeyEvent) -> bool {
        match self {
            KeyBinding::Char(c) => matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(c) && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)),
            KeyBinding::Ctrl(c) => matches!(key.code, KeyCode::Char(k) if k.eq_ignore_ascii_case(c) && key.modifiers.contains(KeyModifiers::CONTROL)),
            KeyBinding::F(n) => matches!(key.code, KeyCode::F(k) if *n == k),
        }
    }
}
