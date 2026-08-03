use std::collections::VecDeque;
use std::io::{self, stdout, Write};
use std::path::Path;
use std::process::{Command, ExitStatus};

use anyhow::Result;
use crossterm::event::DisableMouseCapture;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use rust_i18n::t;
use unicode_width::UnicodeWidthChar;

use crate::app::{App, AppMode, Focus, InputMode};
use crate::cwl;
use crate::theme::Theme;

/// Wrapper around [`CrosstermBackend`] that works around a cursor-positioning bug
/// with multi-width characters (CJK, emoji) in the upstream `0.1.2` backend.
/// It filters out cells that are covered by a previous wide grapheme before
/// delegating the draw to the real backend, preventing extra whitespace and
/// border misalignment in CJK locales.
pub(crate) struct FixedCrosstermBackend<W: Write> {
    inner: CrosstermBackend<W>,
}

impl<W: Write> FixedCrosstermBackend<W> {
    pub(crate) fn new(inner: CrosstermBackend<W>) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for FixedCrosstermBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(&mut self.inner)
    }
}

impl<W: Write> Backend for FixedCrosstermBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut next_pos: Option<Position> = None;
        let mut skip_covered_cells = false;
        let filtered: Vec<(u16, u16, &'a Cell)> = content
            .filter(|(x, y, cell)| {
                if skip_covered_cells
                    && matches!(next_pos, Some(pos) if *y == pos.y && *x < pos.x)
                {
                    return false;
                }
                let width = cell.cell_width();
                next_pos = Some(Position {
                    x: x.saturating_add(width),
                    y: *y,
                });
                skip_covered_cells = width > 1 && !cell.symbol().contains('\u{FE0F}');
                true
            })
            .collect();
        self.inner.draw(filtered.into_iter())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }
}

pub(crate) struct TerminalGuard {
    terminal: Terminal<FixedCrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    pub(crate) fn setup() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(DisableMouseCapture)?;
        let terminal = Terminal::new(FixedCrosstermBackend::new(CrosstermBackend::new(stdout)))?;
        Ok(Self { terminal })
    }

    pub(crate) fn terminal(&mut self) -> &mut Terminal<FixedCrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    pub(crate) fn run_editor(&mut self, path: &Path, editor: &str) -> std::io::Result<()> {
        let mut stdout = stdout();
        disable_raw_mode()?;
        stdout.execute(LeaveAlternateScreen)?;
        stdout.flush()?;

        let result = run_editor_command(path, editor);

        enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;
        self.terminal.clear()?;

        let status = result?;
        if !status.success() {
            return Err(std::io::Error::other(t!("ui.editor_exit_error", status = status.to_string()).to_string()));
        }
        Ok(())
    }
}

fn run_editor_command(path: &Path, editor: &str) -> std::io::Result<ExitStatus> {
    let words = shlex::split(editor).ok_or_else(|| {
        std::io::Error::other(t!("ui.no_editor_configured").to_string())
    })?;
    let mut words = words.into_iter();
    let program = words
        .next()
        .ok_or_else(|| std::io::Error::other(t!("ui.no_editor_configured").to_string()))?;
    if program.contains(std::path::MAIN_SEPARATOR)
        && let Ok(meta) = std::fs::metadata(&program)
        && meta.is_dir()
    {
        return Err(std::io::Error::other(t!("ui.editor_is_directory", path = program.to_string()).to_string()));
    }
    Command::new(&program).args(words).arg(path).status()
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
        let _ = self.terminal.backend_mut().execute(DisableMouseCapture);
        let _ = self.terminal.show_cursor();
    }
}

pub(crate) fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
        eprintln!("{}", t!("ui.panic_message", info = info.to_string()));
    }));
}

pub(crate) fn ui(app: &mut App, frame: &mut Frame) {
    let area = frame.area();
    let layout = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]);
    let [body_area, status_area] = area.layout(&layout);

    let main_area = if app.tab_titles.len() > 1 {
        let layout = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).spacing(1);
        let [tab_area, rest] = body_area.layout(&layout);
        render_tabs(app, frame, tab_area);
        rest
    } else {
        body_area
    };

    if let Some(focus) = app.zoom {
        render_zoomed(app, frame, main_area, focus);
    } else {
        let main_layout = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).spacing(1);
        let [left_area, right_area] = main_area.layout(&main_layout);

        let left_layout = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).spacing(1);
        let [search_area, list_area] = left_area.layout(&left_layout);

        let right_layout = Layout::vertical([Constraint::Percentage(35), Constraint::Percentage(65)]).spacing(1);
        let [history_area, output_area] = right_area.layout(&right_layout);

        render_search(app, frame, search_area);

        match app.app_mode {
            AppMode::Runbook => {
                render_commands(app, frame, list_area);
                render_processes(app, frame, history_area);
                render_log(app, frame, output_area);
            }
            AppMode::Cwl => {
                render_commands(app, frame, list_area);
                render_cwl_info(app, frame, history_area);
                render_log(app, frame, output_area);
            }
            AppMode::Api => {
                let vars_height = (app.variables.len() as u16 + 2).clamp(4, 8);
                let api_layout = Layout::vertical([Constraint::Fill(1), Constraint::Length(vars_height)]).spacing(1);
                let [api_area, variables_area] = list_area.layout(&api_layout);
                render_apis(app, frame, api_area);
                render_variables(app, frame, variables_area);
                render_requests(app, frame, history_area);
                render_api_response(app, frame, output_area);
            }
        }
    }

    if app.input_mode == InputMode::Import {
        render_import(app, frame, body_area);
    }

    if app.show_help {
        render_help(app, frame, body_area);
    }

    if app.input_mode == InputMode::EnvironmentSelect {
        render_environment_select(app, frame, body_area);
    }

    if app.input_mode == InputMode::TabSelect {
        render_tab_select(app, frame, body_area);
    }

    render_message(app, frame, body_area);
    render_status(app, frame, status_area);
}

fn render_message(app: &App, frame: &mut Frame, area: Rect) {
    if app.message.is_empty() {
        return;
    }
    let text_width = app.message.chars().count() as u16;
    let width = (text_width + 4).clamp(20, area.width.saturating_sub(4));
    let height = 3.min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, popup);
    let style = if app.message.starts_with(t!("message.export_failed", error = "").as_ref()) {
        app.theme.error
    } else {
        app.theme.success
    };
    let paragraph = Paragraph::new(app.message.as_str())
        .block(with_cjk_border(Block::default()).borders(Borders::ALL).border_style(style))
        .style(style);
    frame.render_widget(paragraph, popup);
}

fn render_status(app: &App, frame: &mut Frame, area: Rect) {
    if area.height == 0 || app.tab_path.as_os_str().is_empty() {
        return;
    }
    let path = app.tab_path.display().to_string();
    let text = truncate_status(&path, area.width as usize);
    let paragraph = Paragraph::new(text).style(app.theme.dim);
    frame.render_widget(paragraph, area);
}

fn truncate_status(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len <= width {
        return s.to_string();
    }
    if width <= 3 {
        return s.chars().take(width).collect();
    }
    let skip = len.saturating_sub(width - 3);
    format!("...{}", s.chars().skip(skip).collect::<String>())
}

fn render_zoomed(app: &mut App, frame: &mut Frame, area: Rect, focus: Focus) {
    frame.render_widget(Clear, area);
    match (app.app_mode, focus) {
        (AppMode::Runbook, Focus::Commands) => render_commands(app, frame, area),
        (AppMode::Runbook, Focus::Processes) => render_processes(app, frame, area),
        (AppMode::Runbook, Focus::Output) => render_log(app, frame, area),
        (AppMode::Api, Focus::Commands) => render_apis(app, frame, area),
        (AppMode::Api, Focus::Variables) => render_variables(app, frame, area),
        (AppMode::Api, Focus::Processes) => render_requests(app, frame, area),
        (AppMode::Api, Focus::RequestBody) | (AppMode::Api, Focus::ResponseBody) => {
            render_api_response(app, frame, area);
        }
        _ => {}
    }
}

fn render_search(app: &App, frame: &mut Frame, area: Rect) {
    if app.input_mode == InputMode::Import {
        return;
    }
    let search_focused = app.focus == Focus::Commands && app.input_mode == InputMode::Search;
    let hint = match app.app_mode {
        AppMode::Runbook => t!("ui.search_hint.runbook").to_string(),
        AppMode::Api => t!("ui.search_hint.api").to_string(),
        AppMode::Cwl => app
            .cwl_doc
            .as_ref()
            .map(cwl::cwl_summary)
            .unwrap_or_else(|| "CWL".to_string()),
    };
    let text = if app.search.is_empty() { hint.as_str() } else { app.search.as_str() };
    let style = if search_focused { app.theme.input } else { app.theme.border };

    let paragraph = Paragraph::new(text)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.search_title").to_string())
                .borders(Borders::ALL)
                .border_style(style),
        )
        .style(style);
    frame.render_widget(paragraph, area);
}

fn render_import(app: &mut App, frame: &mut Frame, area: Rect) {
    let popup_width = (area.width * 4 / 5).max(30).min(area.width.saturating_sub(4));
    let popup_height = (area.height * 4 / 5).max(12).min(area.height.saturating_sub(4));
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup);
    let block = with_cjk_border(Block::default())
        .title(t!("ui.open_file_title", path = app.import_cwd.display().to_string()).to_string())
        .borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let inner_layout = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).spacing(1);
    let [filter_area, list_area] = inner.layout(&inner_layout);

    let filter_style = if app.import_error {
        app.theme.error
    } else {
        app.theme.focused
    };
    let filter_text = if app.import_filter.is_empty() {
        t!("ui.filter_placeholder").to_string()
    } else {
        app.import_filter.clone()
    };
    let filter_paragraph = Paragraph::new(filter_text)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.filter_title").to_string())
                .borders(Borders::ALL)
                .border_style(filter_style),
        )
        .style(if app.import_filter.is_empty() {
            app.theme.dim
        } else {
            app.theme.border
        });
    frame.render_widget(filter_paragraph, filter_area);

    let items: Vec<ListItem> = app
        .import_entries
        .iter()
        .map(|e| {
            let prefix = if e.is_dir { t!("ui.dir_prefix").to_string() } else { "       ".to_string() };
            ListItem::new(format!("{}{}", prefix, e.name))
        })
        .collect();

    let list_title = app.import_message.clone();
    let list = List::new(items)
        .block(with_cjk_border(Block::default()).title(list_title).borders(Borders::ALL))
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, list_area, &mut app.import_state);
}

fn render_environment_select(app: &mut App, frame: &mut Frame, area: Rect) {
    let popup_width = (area.width * 3 / 5).max(30).min(area.width.saturating_sub(4));
    let popup_height = ((app.environment_choices().len() as u16 + 2).clamp(5, area.height.saturating_sub(4))).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup);
    let block = with_cjk_border(Block::default())
        .title(t!("ui.environment_title", label = app.environment_label()).to_string())
        .borders(Borders::ALL)
        .border_style(app.theme.focused);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let items: Vec<ListItem> = app
        .environment_choices()
        .iter()
        .map(|(name, _)| ListItem::new(name.clone()))
        .collect();

    let list = List::new(items)
        .block(with_cjk_border(Block::default()).borders(Borders::NONE))
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, inner, &mut app.environment_state);
}

fn render_tabs(app: &App, frame: &mut Frame, area: Rect) {
    let selected = app.tab_idx.min(app.tab_titles.len().saturating_sub(1));
    let tabs = Tabs::new(app.tab_titles.clone())
        .select(selected)
        .block(with_cjk_border(Block::default()).borders(Borders::ALL))
        .style(app.theme.border)
        .highlight_style(app.theme.highlight);
    frame.render_widget(tabs, area);
}

fn render_tab_select(app: &mut App, frame: &mut Frame, area: Rect) {
    let popup_width = (area.width * 3 / 5).max(30).min(area.width.saturating_sub(4));
    let popup_height = ((app.tab_titles.len() as u16 + 2).clamp(5, area.height.saturating_sub(4))).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(popup_width)) / 2,
        y: area.y + (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup);
    let block = with_cjk_border(Block::default())
        .title(t!("ui.select_tab_title").to_string())
        .borders(Borders::ALL)
        .border_style(app.theme.focused);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let items: Vec<ListItem> = app
        .tab_titles
        .iter()
        .map(|name| ListItem::new(name.clone()))
        .collect();

    let list = List::new(items)
        .block(with_cjk_border(Block::default()).borders(Borders::NONE))
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, inner, &mut app.tab_select_state);
}

fn render_commands(app: &mut App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&idx| {
            let cmd = &app.commands[idx];
            let key = cmd.key.display();
            let line = Line::from(vec![
                Span::raw(cmd.title.clone()),
                Span::raw("  ["),
                Span::styled(key, app.theme.key),
                Span::raw("]"),
            ]);
            ListItem::new(line)
        })
        .collect();

    let commands_focused = app.focus == Focus::Commands;
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.runbook_title").to_string())
                .borders(Borders::ALL)
                .border_style(if commands_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.command_state);
}

fn render_apis(app: &mut App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .filtered_apis
        .iter()
        .map(|&idx| {
            let api = &app.apis[idx];
            let key = api.key.display();
            let line = Line::from(vec![
                Span::raw(api.method.clone()),
                Span::raw(" "),
                Span::raw(api.name.clone()),
                Span::raw("  ["),
                Span::styled(key, app.theme.key),
                Span::raw("]"),
            ]);
            ListItem::new(line)
        })
        .collect();

    let apis_focused = app.focus == Focus::Commands;
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.apis_title").to_string())
                .borders(Borders::ALL)
                .border_style(if apis_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.api_state);
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        t!("ui.secret_mask").to_string()
    }
}

fn render_variables(app: &mut App, frame: &mut Frame, area: Rect) {
    let editing = app.variable_edit_index;
    let title = if let Some(i) = editing {
        t!(
            "ui.variables_editing",
            name = app.variables.get(i).map(|(n, _)| n.as_str()).unwrap_or("")
        )
        .to_string()
    } else {
        t!(
            "ui.variables_title",
            label = app.environment_label()
        )
        .to_string()
    };

    let items: Vec<ListItem> = app
        .variables
        .iter()
        .enumerate()
        .map(|(i, (name, value))| {
            let secret = app.secret_keys.contains(name);
            let text = if editing == Some(i) {
                let shown = if secret && !app.variable_edit_reveal {
                    mask_secret(&app.variable_edit_value)
                } else {
                    app.variable_edit_value.clone()
                };
                t!("ui.variable_edit_format", name = name.as_str(), value = shown.as_str()).to_string()
            } else {
                let shown = if secret {
                    mask_secret(value)
                } else {
                    value.clone()
                };
                t!("ui.variable_format", name = name.as_str(), value = shown.as_str()).to_string()
            };
            ListItem::new(text)
        })
        .collect();

    let focused = app.focus == Focus::Variables;
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(title)
                .borders(Borders::ALL)
                .border_style(if focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.variable_state);
}

fn render_processes(app: &mut App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .processes
        .iter()
        .map(|p| {
            let (sym, style) = p.status.indicator(&app.theme);
            let sym = cjk_safe_status_symbol(sym);
            let line = Line::from(vec![
                Span::styled(sym, style),
                Span::raw(" "),
                Span::raw(p.title.clone()),
                Span::raw(t!("ui.status_suffix", label = p.status.label()).to_string()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let processes_focused = app.focus == Focus::Processes;
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.processes_title").to_string())
                .borders(Borders::ALL)
                .border_style(if processes_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.process_state);
}

fn render_requests(app: &mut App, frame: &mut Frame, area: Rect) {
    let method_width = app
        .api_requests
        .iter()
        .map(|r| r.method.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    let items: Vec<ListItem> = app
        .api_requests
        .iter()
        .map(|r| {
            let (sym, style) = r.status.indicator(&app.theme);
            let sym = cjk_safe_status_symbol(sym);
            let line = Line::from(vec![
                Span::styled(sym, style),
                Span::raw(" "),
                Span::raw(format!("{:method_width$} {}", r.method, r.url)),
                Span::raw(t!("ui.status_suffix", label = r.status.label()).to_string()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let requests_focused = app.focus == Focus::Processes;
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.requests_title").to_string())
                .borders(Borders::ALL)
                .border_style(if requests_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut app.request_state);
}

fn render_api_response(app: &mut App, frame: &mut Frame, area: Rect) {
    let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).spacing(1);
    let [request_area, response_area] = area.layout(&columns);

    let request_panes = Layout::vertical([Constraint::Length(4), Constraint::Fill(1)]).spacing(1);
    let [request_headers_area, request_body_area] = request_area.layout(&request_panes);

    let response_panes = Layout::vertical([Constraint::Length(4), Constraint::Fill(1)]).spacing(1);
    let [response_headers_area, response_body_area] = response_area.layout(&response_panes);

    let (request_headers_text, request_body_lines) = if let Some(r) = app.selected_request() {
        (
            r.request_headers.clone(),
            colorize_body(
                r.request_content_type.as_deref(),
                &r.request_body,
                request_body_area.width.saturating_sub(2),
                &app.theme,
            ),
        )
    } else {
        (String::new(), vec![Line::from(t!("ui.no_request_selected").to_string())])
    };

    let request_headers_paragraph = Paragraph::new(request_headers_text)
        .block(with_cjk_border(Block::default()).title(t!("ui.request_headers_title").to_string()).borders(Borders::ALL));
    frame.render_widget(request_headers_paragraph, request_headers_area);

    app.request_body_area_height = request_body_area.height.saturating_sub(2);
    app.request_body_area_width = request_body_area.width.saturating_sub(2);

    if request_body_lines.is_empty() {
        app.request_body_state.select(None);
    } else {
        let last = request_body_lines.len() - 1;
        if let Some(selected) = app.request_body_state.selected() {
            if selected > last {
                app.request_body_state.select(Some(last));
            }
        } else {
            app.request_body_state.select(Some(0));
        }
    }

    let request_body_focused = app.focus == Focus::RequestBody;
    let request_items: Vec<ListItem> = request_body_lines
        .into_iter()
        .map(ListItem::new)
        .collect();
    let request_list = List::new(request_items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.request_body_title").to_string())
                .borders(Borders::ALL)
                .border_style(if request_body_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_symbol("");
    frame.render_stateful_widget(request_list, request_body_area, &mut app.request_body_state);

    let (response_headers_text, response_body_lines) = if let Some(r) = app.selected_request() {
        (
            r.headers.clone(),
            colorize_body(
                r.response_content_type.as_deref(),
                &r.body,
                response_body_area.width.saturating_sub(2),
                &app.theme,
            ),
        )
    } else {
        (String::new(), vec![Line::from(t!("ui.no_request_selected").to_string())])
    };

    let response_headers_paragraph = Paragraph::new(response_headers_text)
        .block(with_cjk_border(Block::default()).title(t!("ui.response_headers_title").to_string()).borders(Borders::ALL));
    frame.render_widget(response_headers_paragraph, response_headers_area);

    app.log_area_height = response_body_area.height.saturating_sub(2);

    if response_body_lines.is_empty() {
        app.log_state.select(None);
    } else {
        let last = response_body_lines.len() - 1;
        if app.log_follow {
            app.log_state.select(Some(last));
        } else if let Some(selected) = app.log_state.selected() {
            if selected > last {
                app.log_state.select(Some(last));
            }
        } else {
            app.log_state.select(Some(last));
        }
    }

    let response_body_focused = app.focus == Focus::ResponseBody;
    let response_items: Vec<ListItem> = response_body_lines
        .into_iter()
        .map(ListItem::new)
        .collect();
    let response_list = List::new(response_items)
        .block(
            with_cjk_border(Block::default())
                .title(t!("ui.response_body_title").to_string())
                .borders(Borders::ALL)
                .border_style(if response_body_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_symbol("");
    frame.render_stateful_widget(response_list, response_body_area, &mut app.log_state);
}

fn help_line(key: &str, desc: &str) -> String {
    format!("  {:<22}{}", key, desc)
}

fn help_lines(app: &App) -> Vec<String> {
    let mut lines = vec![
        t!("help.global").to_string(),
        help_line("?", t!("help.desc.toggle_help").as_ref()),
        help_line("Tab", t!("help.desc.cycle_focus_forward").as_ref()),
        help_line("Shift+Tab", t!("help.desc.cycle_focus_backward").as_ref()),
        help_line("m", t!("help.desc.maximize").as_ref()),
        help_line("Ctrl+E", t!("help.desc.export").as_ref()),
        help_line("Ctrl+G", t!("help.desc.switch_env").as_ref()),
        help_line("Ctrl+O", t!("help.desc.open_file").as_ref()),
        help_line("F2", t!("help.desc.switch_tab").as_ref()),
        help_line("Ctrl+Left", t!("help.desc.prev_tab").as_ref()),
        help_line("Ctrl+Right", t!("help.desc.next_tab").as_ref()),
        help_line("F3", t!("help.desc.new_tab").as_ref()),
        help_line("F4", t!("help.desc.edit_source").as_ref()),
        help_line("Ctrl+T", t!("help.desc.cycle_theme").as_ref()),
        help_line("Ctrl+C", t!("help.desc.quit").as_ref()),
        help_line("Ctrl+N", t!("help.desc.move_down").as_ref()),
        help_line("Ctrl+P", t!("help.desc.move_up").as_ref()),
        String::new(),
    ];

    match app.app_mode {
        AppMode::Runbook | AppMode::Cwl => match app.focus {
            Focus::Commands => {
                lines.push(t!("help.section.runbook_commands").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.select_command").as_ref()));
                lines.push(help_line("Enter", t!("help.desc.run_command").as_ref()));
                lines.push(help_line("/", t!("help.desc.search_commands").as_ref()));
                lines.push(help_line("Esc", t!("help.desc.clear_search").as_ref()));
                lines.push(help_line("letter key", t!("help.desc.run_by_key").as_ref()));
            }
            Focus::Processes => {
                lines.push(t!("help.section.runbook_processes").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.select_process").as_ref()));
                lines.push(help_line("PgUp/PgDn/Home/End", t!("help.desc.scroll_output").as_ref()));
                lines.push(help_line("Esc/q", t!("help.desc.back_to_commands").as_ref()));
            }
            Focus::Output => {
                lines.push(t!("help.section.runbook_output").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.scroll_output").as_ref()));
                lines.push(help_line("PgUp/PgDn/Home/End", t!("help.desc.scroll_output").as_ref()));
                lines.push(help_line("Esc/q", t!("help.desc.back_to_processes").as_ref()));
            }
            _ => {}
        },
        AppMode::Api => match app.focus {
            Focus::Commands => {
                lines.push(t!("help.section.api_apis").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.select_api").as_ref()));
                lines.push(help_line("Enter", t!("help.desc.send_request").as_ref()));
                lines.push(help_line("/", t!("help.desc.search_apis").as_ref()));
                lines.push(help_line("Esc", t!("help.desc.clear_search").as_ref()));
                lines.push(help_line("letter key", t!("help.desc.send_by_key").as_ref()));
            }
            Focus::Variables => {
                lines.push(t!("help.section.api_variables").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.select_variable").as_ref()));
                lines.push(help_line("Enter", t!("help.desc.edit_value").as_ref()));
                lines.push(help_line("Esc", t!("help.desc.cancel_edit").as_ref()));
            }
            Focus::Processes => {
                lines.push(t!("help.section.api_requests").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.select_request").as_ref()));
                lines.push(help_line("PgUp/PgDn/Home/End", t!("help.desc.scroll_response_body").as_ref()));
                lines.push(help_line("Esc/q", t!("help.desc.back_to_apis").as_ref()));
            }
            Focus::RequestBody => {
                lines.push(t!("help.section.api_request_body").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.scroll_request_body").as_ref()));
                lines.push(help_line("PgUp/PgDn/Home/End", t!("help.desc.scroll_request_body").as_ref()));
                lines.push(help_line("Esc/q", t!("help.desc.back_to_requests").as_ref()));
            }
            Focus::ResponseBody => {
                lines.push(t!("help.section.api_response_body").to_string());
                lines.push(help_line("↑/↓ or Ctrl+P/Ctrl+N", t!("help.desc.scroll_response_body").as_ref()));
                lines.push(help_line("PgUp/PgDn/Home/End", t!("help.desc.scroll_response_body").as_ref()));
                lines.push(help_line("Esc/q", t!("help.desc.back_to_requests").as_ref()));
            }
            _ => {}
        },
    }
    lines
}

fn render_help(app: &App, frame: &mut Frame, area: Rect) {
    let lines = help_lines(app);
    let height = (lines.len() as u16 + 4).min(area.height * 3 / 4).max(10);
    let width = (area.width * 3 / 4).min(area.width.saturating_sub(4)).max(20);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, popup);
    let items: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
    let list = List::new(items)
        .block(with_cjk_border(Block::default()).title(t!("ui.help_title").to_string()).borders(Borders::ALL));
    frame.render_widget(list, popup);
}

fn render_cwl_info(app: &App, frame: &mut Frame, area: Rect) {
    let summary = app
        .cwl_doc
        .as_ref()
        .map(cwl::cwl_summary)
        .unwrap_or_else(|| "CWL".to_string());

    let mut text = summary;
    if let Some(doc) = &app.cwl_doc {
        text.push('\n');
        for (id, _) in cwl::cwl_inputs(doc) {
            text.push('\n');
            text.push_str(&format!("- {id}"));
        }
    }

    let paragraph = Paragraph::new(text)
        .block(
            with_cjk_border(Block::default().title("CWL").borders(Borders::ALL))
                .border_style(app.theme.border),
        )
        .style(app.theme.border);
    frame.render_widget(paragraph, area);
}

fn render_log(app: &mut App, frame: &mut Frame, area: Rect) {
    app.log_area_height = area.height.saturating_sub(2);

    let (title, lines) = if let Some(p) = app.selected_process() {
        let width = area.width.saturating_sub(2);
        let display = wrap_lines(&p.output, width);
        (t!("ui.output_title", title = p.title.clone()).to_string(), display)
    } else {
        (t!("ui.output_empty_title").to_string(), vec![t!("ui.no_process_selected").to_string()])
    };

    if lines.is_empty() {
        app.log_state.select(None);
    } else {
        let last = lines.len() - 1;
        if app.log_follow {
            app.log_state.select(Some(last));
        } else if let Some(selected) = app.log_state.selected() {
            if selected > last {
                app.log_state.select(Some(last));
            }
        } else {
            app.log_state.select(Some(last));
        }
    }

    let output_focused = app.focus == Focus::Output;
    let items: Vec<ListItem> = lines.into_iter().map(ListItem::new).collect();
    let list = List::new(items)
        .block(
            with_cjk_border(Block::default())
                .title(title)
                .borders(Borders::ALL)
                .border_style(if output_focused {
                    app.theme.focused
                } else {
                    app.theme.border
                }),
        )
        .highlight_symbol("");

    frame.render_stateful_widget(list, area, &mut app.log_state);
}

fn is_content_type(content_type: Option<&str>, kind: &str) -> bool {
    content_type
        .map(|ct| ct.contains(kind))
        .unwrap_or(false)
}

fn colorize_line(content_type: Option<&str>, line: &str, theme: &Theme) -> Line<'static> {
    if is_content_type(content_type, "json") || is_content_type(content_type, "javascript") {
        colorize_json_line(line, theme)
    } else if is_content_type(content_type, "xml") || is_content_type(content_type, "html") {
        colorize_xml_line(line, theme)
    } else {
        Line::from(line.to_string())
    }
}

fn colorize_json_line(line: &str, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            let mut text = c.to_string();
            while let Some(&nc) = chars.peek() {
                if nc.is_whitespace() {
                    text.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            spans.push(Span::raw(text));
        } else if c == '"' {
            let mut text = String::from(c);
            while let Some(nc) = chars.next() {
                text.push(nc);
                if nc == '\\' {
                    if let Some(ec) = chars.next() {
                        text.push(ec);
                    }
                } else if nc == '"' {
                    break;
                }
            }
            spans.push(Span::styled(text, theme.json_string));
        } else if c == '{' || c == '}' || c == '[' || c == ']' || c == ',' || c == ':' {
            spans.push(Span::styled(c.to_string(), theme.json_punctuation));
        } else if c.is_ascii_digit() || c == '-' {
            let mut text = c.to_string();
            while let Some(&nc) = chars.peek() {
                if nc.is_ascii_digit()
                    || nc == '.'
                    || nc == 'e'
                    || nc == 'E'
                    || nc == '+'
                    || nc == '-'
                {
                    text.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            spans.push(Span::styled(text, theme.json_number));
        } else if c.is_alphabetic() {
            let mut text = c.to_string();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphabetic() {
                    text.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            let style = if text == "true" || text == "false" || text == "null" {
                theme.json_bool
            } else {
                theme.border
            };
            spans.push(Span::styled(text, style));
        } else {
            spans.push(Span::raw(c.to_string()));
        }
    }
    Line::from(spans)
}

fn colorize_xml_line(line: &str, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            if !text.is_empty() {
                spans.push(Span::raw(text));
                text = String::new();
            }
            let mut tag = String::from(c);
            let mut in_quote = None;
            for nc in chars.by_ref() {
                tag.push(nc);
                if in_quote.is_none() {
                    if nc == '"' || nc == '\'' {
                        in_quote = Some(nc);
                    } else if nc == '>' {
                        break;
                    }
                } else if Some(nc) == in_quote {
                    in_quote = None;
                }
            }
            spans.push(Span::styled(tag, theme.xml_tag));
        } else {
            text.push(c);
        }
    }
    if !text.is_empty() {
        spans.push(Span::raw(text));
    }
    Line::from(spans)
}

fn wrap_colored_lines<'a>(lines: Vec<Line<'a>>, width: u16) -> Vec<Line<'static>> {
    let width = width as usize;
    if width == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut current_line = Line::default();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    let mut current_width = 0;

    for line in lines {
        for span in line.spans {
            for grapheme in span.styled_graphemes(Style::default()) {
                let gw = grapheme.symbol.cell_width() as usize;
                if gw > 0 && current_width + gw > width {
                    if let Some(style) = current_style {
                        current_line.spans.push(Span::styled(current_text, style));
                    }
                    result.push(current_line);
                    current_line = Line::default();
                    current_text = String::new();
                    current_style = None;
                    current_width = 0;
                }
                if current_style != Some(grapheme.style) {
                    if let Some(style) = current_style {
                        current_line.spans.push(Span::styled(current_text, style));
                    }
                    current_text = grapheme.symbol.to_string();
                    current_style = Some(grapheme.style);
                } else {
                    current_text.push_str(grapheme.symbol);
                }
                current_width += gw;
            }
        }
    }
    if let Some(style) = current_style {
        current_line.spans.push(Span::styled(current_text, style));
    }
    if !current_line.spans.is_empty() {
        result.push(current_line);
    }
    result
}

pub(crate) fn colorize_body(
    content_type: Option<&str>,
    lines: &VecDeque<String>,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let source: Vec<Line> = lines
        .iter()
        .map(|l| colorize_line(content_type, l, theme))
        .collect();
    wrap_colored_lines(source, width)
}

fn is_cjk_locale() -> bool {
    let locale = rust_i18n::locale();
    let locale = &*locale;
    locale.starts_with("zh") || locale.starts_with("ja") || locale.starts_with("ko")
}

/// CJK terminals may render status symbols with ambiguous East-Asian width
/// (e.g. `●`, `✓`, `✗` may be two cells). Use ASCII alternatives in CJK
/// locales so ratatui and the terminal agree on the symbol width.
fn cjk_safe_status_symbol(sym: &str) -> &str {
    if !is_cjk_locale() {
        return sym;
    }
    match sym {
        "●" => "*",
        "✓" => "+",
        "✗" => "!",
        _ => sym,
    }
}

const ASCII_BORDER: ratatui::symbols::border::Set<'static> = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

fn with_cjk_border<'a>(mut block: ratatui::widgets::Block<'a>) -> ratatui::widgets::Block<'a> {
    if is_cjk_locale() {
        block = block.border_set(ASCII_BORDER);
    }
    block
}

fn wrap_lines(output: &VecDeque<String>, width: u16) -> Vec<String> {
    let width = width as usize;
    if width == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for line in output {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0usize;
        for c in line.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if current_width + cw > width && !current.is_empty() {
                result.push(current);
                current = String::new();
                current_width = 0;
            }
            current.push(c);
            current_width += cw;
        }
        if !current.is_empty() || line.is_empty() {
            result.push(current);
        }
    }
    result
}
