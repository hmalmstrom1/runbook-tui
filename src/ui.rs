use std::collections::VecDeque;
use std::io::{self, stdout};

use anyhow::Result;
use crossterm::event::DisableMouseCapture;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthChar;

use crate::app::{App, AppMode, Focus, InputMode};
use crate::theme::Theme;

pub(crate) struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    pub(crate) fn setup() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(DisableMouseCapture)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    pub(crate) fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }
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
        eprintln!("Application panicked: {info}");
    }));
}

pub(crate) fn ui(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

    if let Some(focus) = app.zoom {
        render_zoomed(app, frame, area, focus);
    } else {
        let main_layout = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).spacing(1);
        let [left_area, right_area] = area.layout(&main_layout);

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
        render_import(app, frame, area);
    }

    if app.show_help {
        render_help(app, frame, area);
    }

    if app.input_mode == InputMode::EnvironmentSelect {
        render_environment_select(app, frame, area);
    }

    render_message(app, frame, area);
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
    let style = if app.message.starts_with("Export failed") {
        app.theme.error
    } else {
        app.theme.success
    };
    let paragraph = Paragraph::new(app.message.as_str())
        .block(Block::default().borders(Borders::ALL).border_style(style))
        .style(style);
    frame.render_widget(paragraph, popup);
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
        AppMode::Runbook => "Type / to search, Enter to run, Ctrl+E export, Ctrl+O open file, Tab/Shift-Tab panes, PgUp/Dn scroll output",
        AppMode::Api => "Type / to search APIs, Enter to send/edit, Ctrl+E export, Ctrl+G env, Ctrl+O open file, Tab/Shift-Tab panes, PgUp/Dn scroll body",
    };
    let text = if app.search.is_empty() { hint } else { &app.search };
    let style = if search_focused { app.theme.input } else { app.theme.border };

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title("Search")
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
    let block = Block::default()
        .title(format!("Open File - {}", app.import_cwd.display()))
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
        "Type to filter...".to_string()
    } else {
        app.import_filter.clone()
    };
    let filter_paragraph = Paragraph::new(filter_text)
        .block(
            Block::default()
                .title("Filter")
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
            let prefix = if e.is_dir { "[dir]  " } else { "       " };
            ListItem::new(format!("{}{}", prefix, e.name))
        })
        .collect();

    let list_title = app.import_message.clone();
    let list = List::new(items)
        .block(Block::default().title(list_title).borders(Borders::ALL))
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
    let block = Block::default()
        .title(format!("Environment (current: {})", app.environment_label()))
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
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(app.theme.highlight)
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, inner, &mut app.environment_state);
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
            Block::default()
                .title("Runbook")
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
            Block::default()
                .title("APIs")
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
        "********".to_string()
    }
}

fn render_variables(app: &mut App, frame: &mut Frame, area: Rect) {
    let editing = app.variable_edit_index;
    let title = if let Some(i) = editing {
        format!(
            "Editing: {}",
            app.variables.get(i).map(|(n, _)| n.as_str()).unwrap_or("")
        )
    } else {
        format!(
            "Variables (env: {}) (Enter to edit)",
            app.environment_label()
        )
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
                format!("{}: {}", name, shown)
            } else {
                let shown = if secret {
                    mask_secret(value)
                } else {
                    value.clone()
                };
                format!("{} = {}", name, shown)
            };
            ListItem::new(text)
        })
        .collect();

    let focused = app.focus == Focus::Variables;
    let list = List::new(items)
        .block(
            Block::default()
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
            let line = Line::from(vec![
                Span::styled(sym, style),
                Span::raw(" "),
                Span::raw(p.title.clone()),
                Span::raw(format!(" ({})", p.status.label())),
            ]);
            ListItem::new(line)
        })
        .collect();

    let processes_focused = app.focus == Focus::Processes;
    let list = List::new(items)
        .block(
            Block::default()
                .title("Processes")
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
    let items: Vec<ListItem> = app
        .api_requests
        .iter()
        .map(|r| {
            let (sym, style) = r.status.indicator(&app.theme);
            let line = Line::from(vec![
                Span::styled(sym, style),
                Span::raw(" "),
                Span::raw(format!("{} {}", r.method, r.url)),
                Span::raw(format!(" ({})", r.status.label())),
            ]);
            ListItem::new(line)
        })
        .collect();

    let requests_focused = app.focus == Focus::Processes;
    let list = List::new(items)
        .block(
            Block::default()
                .title("Requests")
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
        (String::new(), vec![Line::from("No request selected")])
    };

    let request_headers_paragraph = Paragraph::new(request_headers_text)
        .block(Block::default().title("Request Headers").borders(Borders::ALL));
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
            Block::default()
                .title("Request Body")
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
        (String::new(), vec![Line::from("No request selected")])
    };

    let response_headers_paragraph = Paragraph::new(response_headers_text)
        .block(Block::default().title("Response Headers").borders(Borders::ALL));
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
            Block::default()
                .title("Response Body")
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

fn help_lines(app: &App) -> Vec<String> {
    let mut lines = vec![
        "Global:".to_string(),
        "  ?            toggle this help".to_string(),
        "  Tab          cycle focus forward".to_string(),
        "  Shift+Tab    cycle focus backward".to_string(),
        "  m            maximize / restore focused pane".to_string(),
        "  Ctrl+E       export selected output".to_string(),
        "  Ctrl+G       switch API environment".to_string(),
        "  Ctrl+O       open file".to_string(),
        "  Ctrl+C       quit".to_string(),
        "  Ctrl+N       move down / next".to_string(),
        "  Ctrl+P       move up / previous".to_string(),
        "".to_string(),
    ];

    match app.app_mode {
        AppMode::Runbook => match app.focus {
            Focus::Commands => {
                lines.push("Runbook - Commands:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  select command".to_string());
                lines.push("  Enter                 run selected command".to_string());
                lines.push("  /                     search commands".to_string());
                lines.push("  Esc                   clear search".to_string());
                lines.push("  letter key            run command by keybinding".to_string());
            }
            Focus::Processes => {
                lines.push("Runbook - Processes:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  select process".to_string());
                lines.push("  PgUp/PgDn/Home/End    scroll output".to_string());
                lines.push("  Esc/q                 back to commands".to_string());
            }
            Focus::Output => {
                lines.push("Runbook - Output:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  scroll output".to_string());
                lines.push("  PgUp/PgDn/Home/End    scroll output".to_string());
                lines.push("  Esc/q                 back to processes".to_string());
            }
            _ => {}
        },
        AppMode::Api => match app.focus {
            Focus::Commands => {
                lines.push("API - APIs:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  select API".to_string());
                lines.push("  Enter                 send selected request".to_string());
                lines.push("  /                     search APIs".to_string());
                lines.push("  Esc                   clear search".to_string());
                lines.push("  letter key            send API by keybinding".to_string());
            }
            Focus::Variables => {
                lines.push("API - Variables:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  select variable".to_string());
                lines.push("  Enter                 edit selected value".to_string());
                lines.push("  Esc                   cancel edit".to_string());
            }
            Focus::Processes => {
                lines.push("API - Requests:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  select request".to_string());
                lines.push("  PgUp/PgDn/Home/End    scroll response body".to_string());
                lines.push("  Esc/q                 back to APIs".to_string());
            }
            Focus::RequestBody => {
                lines.push("API - Request Body:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  scroll request body".to_string());
                lines.push("  PgUp/PgDn/Home/End    scroll request body".to_string());
                lines.push("  Esc/q                 back to requests".to_string());
            }
            Focus::ResponseBody => {
                lines.push("API - Response Body:".to_string());
                lines.push("  ↑/↓ or Ctrl+P/Ctrl+N  scroll response body".to_string());
                lines.push("  PgUp/PgDn/Home/End    scroll response body".to_string());
                lines.push("  Esc/q                 back to requests".to_string());
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
        .block(Block::default().title("Help").borders(Borders::ALL));
    frame.render_widget(list, popup);
}

fn render_log(app: &mut App, frame: &mut Frame, area: Rect) {
    app.log_area_height = area.height.saturating_sub(2);

    let (title, lines) = if let Some(p) = app.selected_process() {
        let width = area.width.saturating_sub(2);
        let display = wrap_lines(&p.output, width);
        (format!("Output: {}", p.title), display)
    } else {
        ("Output".to_string(), vec!["No process selected".to_string()])
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
            Block::default()
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
