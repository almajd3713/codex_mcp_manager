use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::config::{OverrideState, ServerField, ServerOrigin, TransportKind};

use super::server_form::{form_field_values, tool_values};
use super::text_edit;
use super::{
    AddStep, AddWizard, App, EditorState, FieldInputState, InputState, InspectBack, InspectorState,
    Screen, ServerForm, ToolListState, ToolNameInputState, WizardTransport,
};

pub(super) fn render(frame: &mut Frame<'_>, app: &App<'_>) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " codex-mcp ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("MCP profile launcher"),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, chunks[0]);

    match &app.screen {
        Screen::Editor(editor) => {
            render_editor(frame, chunks[1], editor);
        }
        Screen::Inventory { selected } => {
            render_inventory(frame, chunks[1], app, *selected);
        }
        Screen::AddServer(wizard) => {
            render_add_server(frame, chunks[1], wizard);
        }
        Screen::ServerForm(form) | Screen::FieldInput(FieldInputState { form, .. }) => {
            render_server_form(frame, chunks[1], app, form);
        }
        Screen::ToolList(list) | Screen::ToolNameInput(ToolNameInputState { list, .. }) => {
            render_tool_list(frame, chunks[1], list);
        }
        Screen::ConfirmInspect { back, .. } => match back {
            InspectBack::Profiles => {
                render_profiles(frame, chunks[1], app);
            }
            InspectBack::Inventory { selected } => {
                render_inventory(frame, chunks[1], app, *selected);
            }
        },
        Screen::ConfirmEdit { selected, .. } => {
            render_inventory(frame, chunks[1], app, *selected);
        }
        Screen::Inspector(inspector) => {
            render_inspector(frame, chunks[1], inspector);
        }
        _ => render_profiles(frame, chunks[1], app),
    }

    let footer = match &app.screen {
        Screen::Profiles => {
            "↑/↓ navigate  Enter launch  n new  e edit  i inspect TOML  m MCP inventory"
        }
        Screen::Editor(_) => "↑/↓ navigate  Space cycle state  s save  Esc cancel",
        Screen::Inventory { .. } => {
            "↑/↓ navigate  n add server  e edit  i inspect TOML  v validate  Esc profiles"
        }
        Screen::AddServer(wizard) if wizard.step == AddStep::AssignProfiles => {
            "↑/↓ navigate  Space toggle profile  s/Enter save  Esc cancel"
        }
        Screen::AddServer(wizard) if wizard.step == AddStep::Transport => {
            "↑/↓ or s/h choose transport  Enter continue  Esc cancel"
        }
        Screen::AddServer(_) => {
            "←/→ move  Home/End  Backspace/Delete edit  Enter continue  Ctrl-U clear  Esc cancel"
        }
        Screen::ServerForm(_) => {
            "↑/↓ fields  Enter edit  Space cycle  Delete unset  s save  Esc cancel"
        }
        Screen::FieldInput(_) => {
            "←/→ move  Home/End  Backspace/Delete edit  Enter apply  Ctrl-U clear  Esc cancel"
        }
        Screen::ToolList(_) => {
            "↑/↓ tools  a add  Enter/e edit  d delete  Space unset/empty  Esc fields"
        }
        Screen::ToolNameInput(_) => {
            "←/→ move  Home/End  Backspace/Delete edit  Enter apply  Ctrl-U clear  Esc cancel"
        }
        Screen::Input(_) => {
            "←/→ move  Home/End  Backspace/Delete edit  Enter confirm  Ctrl-U clear  Esc cancel"
        }
        Screen::ConfirmDelete { .. } => "y move to trash  n/Esc cancel",
        Screen::ConfirmInspect { .. } => "y show raw TOML  n/Esc cancel",
        Screen::ConfirmEdit { .. } => "y load values into editor  n/Esc cancel",
        Screen::Inspector(_) => "↑/↓ PgUp/PgDn scroll  ←/→ horizontal  Home/End  q/Esc back",
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[2],
    );

    match &app.screen {
        Screen::Input(input) => render_input(frame, area, input),
        Screen::FieldInput(input) => render_field_input(frame, area, input),
        Screen::ToolNameInput(input) => render_tool_name_input(frame, area, input),
        Screen::ConfirmDelete { profile_name } => {
            render_confirmation(frame, area, profile_name);
        }
        Screen::ConfirmInspect { .. } => {
            render_inspect_confirmation(frame, area);
        }
        Screen::ConfirmEdit { server_name, .. } => {
            render_edit_confirmation(frame, area, server_name);
        }
        _ => {}
    }
    if let Some(notice) = &app.notice {
        render_notice(frame, area, notice);
    }
}

fn render_inspector(frame: &mut Frame<'_>, area: Rect, inspector: &InspectorState) {
    let source = if inspector.source.is_empty() {
        "(empty TOML file)"
    } else {
        inspector.source.as_str()
    };
    let paragraph = Paragraph::new(source)
        .block(
            Block::default()
                .title(format!(" {} — raw, read-only ", inspector.title))
                .borders(Borders::ALL),
        )
        .scroll((inspector.vertical_scroll, inspector.horizontal_scroll));
    frame.render_widget(paragraph, area);
}

fn render_inventory(frame: &mut Frame<'_>, area: Rect, app: &App<'_>, selected: usize) {
    let items = if app.catalog.base_servers.is_empty() {
        vec![ListItem::new(Span::styled(
            "No base MCP servers. Press n to add one.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.catalog
            .base_servers
            .iter()
            .map(|server| {
                let transport = match server.transport {
                    TransportKind::Stdio => "stdio",
                    TransportKind::Http => "http",
                    TransportKind::Invalid => "invalid",
                };
                let status = if !server.valid {
                    ("invalid", Color::Red)
                } else if server.effective_enabled {
                    ("enabled in Base", Color::Green)
                } else {
                    ("disabled in Base", Color::Yellow)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        server.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {transport}  "),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(status.0, Style::default().fg(status.1)),
                ]))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(" MCP Inventory — base definitions only ")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    if !app.catalog.base_servers.is_empty() {
        state.select(Some(selected));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_add_server(frame: &mut Frame<'_>, area: Rect, wizard: &AddWizard) {
    match wizard.step {
        AddStep::Transport => {
            render_transport_choice(frame, area, wizard);
        }
        AddStep::AssignProfiles => {
            render_profile_choices(frame, area, wizard);
        }
        _ => render_wizard_input(frame, area, wizard),
    }
}

fn render_transport_choice(frame: &mut Frame<'_>, area: Rect, wizard: &AddWizard) {
    let selected = match wizard.transport {
        WizardTransport::Stdio => 0,
        WizardTransport::Http => 1,
    };
    let items = vec![
        ListItem::new(Line::from(vec![
            Span::styled(
                "Local / stdio",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  executable plus argument list",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Remote HTTP", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                "  streamable HTTP endpoint",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
    ];
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " Add MCP server '{}' — choose transport ",
                    wizard.name
                ))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_profile_choices(frame: &mut Frame<'_>, area: Rect, wizard: &AddWizard) {
    let items = if wizard.choices.is_empty() {
        vec![ListItem::new(Span::styled(
            "No profiles exist. The server will be added disabled in Base.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        wizard
            .choices
            .iter()
            .map(|choice| {
                let marker = if !choice.available {
                    "!"
                } else if choice.selected {
                    "✓"
                } else {
                    " "
                };
                let suffix = if choice.available {
                    ""
                } else {
                    "  [invalid profile]"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("[{marker}] "),
                        Style::default().fg(if choice.selected {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::raw(choice.name.clone()),
                    Span::styled(suffix, Style::default().fg(Color::Red)),
                ]))
            })
            .collect()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(
                    " Enable '{}' in profiles — Base remains disabled ",
                    wizard.name
                ))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    if !wizard.choices.is_empty() {
        state.select(Some(wizard.selected_choice));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_wizard_input(frame: &mut Frame<'_>, area: Rect, wizard: &AddWizard) {
    let (title, help) = match wizard.step {
        AddStep::Name => (
            " Add MCP server — name ",
            "Letters, numbers, hyphens, and underscores only.",
        ),
        AddStep::Transport | AddStep::AssignProfiles => {
            unreachable!()
        }
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "Creating '{}'. Values typed in this wizard are only shown while editing.",
            if wizard.name.is_empty() {
                "a new server"
            } else {
                &wizard.name
            }
        ))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center),
        chunks[0],
    );
    let input_width = chunks[1].width.saturating_sub(2);
    let (visible, cursor_column) = text_edit::viewport(&wizard.input, wizard.cursor, input_width);
    frame.render_widget(
        Paragraph::new(visible)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(help)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[2],
    );
    let cursor_x = chunks[1]
        .x
        .saturating_add(1)
        .saturating_add(cursor_column)
        .min(chunks[1].right().saturating_sub(2));
    frame.set_cursor_position((cursor_x, chunks[1].y.saturating_add(1)));
}

fn render_server_form(frame: &mut Frame<'_>, area: Rect, app: &App<'_>, form: &ServerForm) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(4)])
        .split(area);
    let base_state = app
        .catalog
        .base_servers
        .iter()
        .find(|server| server.name == form.editable.server.name)
        .map(|server| {
            if server.effective_enabled {
                "enabled"
            } else {
                "disabled"
            }
        })
        .unwrap_or("will be disabled after creation");
    frame.render_widget(
        Paragraph::new(format!(
            "{}  •  Base: {base_state}  •  transport and Base enablement are read-only here",
            form.editable.server.name
        ))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center),
        chunks[0],
    );

    let values = form_field_values(form);
    let mut items = Vec::new();
    let mut selected_row = 0;
    let mut previous_section = "";
    for (index, field) in form.fields.iter().enumerate() {
        let section = field_section(*field);
        if section != previous_section {
            items.push(ListItem::new(Span::styled(
                format!("  {section}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            previous_section = section;
        }
        if index == form.selected {
            selected_row = items.len();
        }
        let dirty = if form.dirty_fields.contains(field) {
            "*"
        } else {
            " "
        };
        let locked = if form.is_locked(*field) {
            "  [read-only]"
        } else {
            ""
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!("{dirty} {:<28}", field_label(*field)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(values.get(field).cloned().unwrap_or_default()),
            Span::styled(locked, Style::default().fg(Color::Yellow)),
        ])));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .title(" MCP server fields — * changed ")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(selected_row));
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

fn render_tool_list(frame: &mut Frame<'_>, area: Rect, list: &ToolListState) {
    let configured = match list.field {
        ServerField::EnabledTools => &list.form.editable.server.options.enabled_tools,
        ServerField::DisabledTools => &list.form.editable.server.options.disabled_tools,
        _ => unreachable!(),
    };
    let status = match configured {
        None => "unset — Codex default behavior",
        Some(values) if values.is_empty() => "explicit empty list",
        Some(_) => "explicit tool list",
    };
    let values = tool_values(&list.form, list.field);
    let items = if values.is_empty() {
        vec![ListItem::new(Span::styled(
            format!("No entries ({status}). Press a to add or Space to toggle unset/empty."),
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        values
            .iter()
            .map(|value| ListItem::new(value.clone()))
            .collect()
    };
    let widget = List::new(items)
        .block(
            Block::default()
                .title(format!(" {} — {status} ", field_label(list.field)))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    if !values.is_empty() {
        state.select(Some(list.selected));
    }
    frame.render_stateful_widget(widget, area, &mut state);
}

fn render_field_input(frame: &mut Frame<'_>, area: Rect, input: &FieldInputState) {
    render_text_popup(
        frame,
        area,
        &format!(" {} ", field_label(input.field)),
        &input.value,
        input.cursor,
    );
}

fn render_tool_name_input(frame: &mut Frame<'_>, area: Rect, input: &ToolNameInputState) {
    let action = if input.edit_index.is_some() {
        "Edit"
    } else {
        "Add"
    };
    render_text_popup(
        frame,
        area,
        &format!(" {action} tool name "),
        &input.value,
        input.cursor,
    );
}

fn render_text_popup(frame: &mut Frame<'_>, area: Rect, title: &str, value: &str, cursor: usize) {
    let popup = centered_rect(area, 68, 5);
    let input_width = popup.width.saturating_sub(2);
    let (visible, cursor_column) = text_edit::viewport(value, cursor, input_width);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(visible)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        popup,
    );
    let cursor_x = popup
        .x
        .saturating_add(1)
        .saturating_add(cursor_column)
        .min(popup.right().saturating_sub(2));
    frame.set_cursor_position((cursor_x, popup.y.saturating_add(1)));
}

fn field_section(field: ServerField) -> &'static str {
    match field {
        ServerField::Command
        | ServerField::Arguments
        | ServerField::WorkingDirectory
        | ServerField::EnvironmentVariables
        | ServerField::Url
        | ServerField::BearerEnvironmentVariable => "CONNECTION",
        ServerField::Required | ServerField::StartupTimeout | ServerField::ToolTimeout => "RUNTIME",
        ServerField::EnabledTools
        | ServerField::DisabledTools
        | ServerField::DefaultToolsApprovalMode => "TOOL POLICY",
    }
}

fn field_label(field: ServerField) -> &'static str {
    match field {
        ServerField::Command => "Command",
        ServerField::Arguments => "Arguments",
        ServerField::WorkingDirectory => "Working directory",
        ServerField::EnvironmentVariables => "Inherited env vars",
        ServerField::Url => "URL",
        ServerField::BearerEnvironmentVariable => "Bearer token env var",
        ServerField::Required => "Required",
        ServerField::StartupTimeout => "Startup timeout",
        ServerField::ToolTimeout => "Tool timeout",
        ServerField::EnabledTools => "Enabled tools",
        ServerField::DisabledTools => "Disabled tools",
        ServerField::DefaultToolsApprovalMode => "Default approval mode",
    }
}

fn render_profiles(frame: &mut Frame<'_>, area: Rect, app: &App<'_>) {
    let base_enabled = app
        .catalog
        .base_servers
        .iter()
        .filter(|server| server.effective_enabled && server.valid)
        .count();
    let mut items = vec![ListItem::new(Line::from(vec![
        Span::styled("Base", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(
                "  {base_enabled}/{} MCP servers enabled",
                app.catalog.base_servers.len()
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]))];

    for profile in &app.catalog.profiles {
        let status = if profile.error.is_some() {
            Span::styled("  invalid configuration", Style::default().fg(Color::Red))
        } else {
            Span::styled(
                format!(
                    "  {}/{} MCP servers enabled",
                    profile.enabled_count(),
                    profile.servers.len()
                ),
                Style::default().fg(Color::DarkGray),
            )
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                profile.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            status,
        ])));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" Profiles — {} ", app.store.home().display()))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, editor: &EditorState) {
    let items = if editor.servers.is_empty() {
        vec![ListItem::new(Span::styled(
            "No MCP servers are defined in the base config or this profile.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        editor
            .servers
            .iter()
            .map(|server| {
                let (label, color) = match server.state {
                    OverrideState::Inherited => (
                        format!(
                            "inherited ({})",
                            if server.inherited_enabled {
                                "on"
                            } else {
                                "off"
                            }
                        ),
                        Color::DarkGray,
                    ),
                    OverrideState::Enabled => ("enabled".to_owned(), Color::Green),
                    OverrideState::Disabled => ("disabled".to_owned(), Color::Yellow),
                };
                let origin = match server.origin {
                    ServerOrigin::Base => "",
                    ServerOrigin::Profile => "  [profile-local]",
                    ServerOrigin::OverrideOnly => "  [missing definition]",
                };
                let validity = if server.valid { "" } else { "  [invalid]" };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        server.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(label, Style::default().fg(color)),
                    Span::styled(
                        format!("{origin}{validity}"),
                        Style::default().fg(Color::Red),
                    ),
                ]))
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" Edit profile: {} ", editor.profile_name))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = ListState::default();
    if !editor.servers.is_empty() {
        state.select(Some(editor.selected));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_input(frame: &mut Frame<'_>, area: Rect, input: &InputState) {
    render_text_popup(frame, area, input.title, &input.value, input.cursor);
}

fn render_confirmation(frame: &mut Frame<'_>, area: Rect, profile_name: &str) {
    let popup = centered_rect(area, 68, 5);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!(
            "Move profile '{profile_name}' to .codex-mcp-trash? (y/N)"
        ))
        .block(
            Block::default()
                .title(" Delete profile ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_inspect_confirmation(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(area, 76, 8);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(
            "Raw source may contain commands, URLs, headers, or credentials. Show it? (y/N)",
        )
        .block(
            Block::default()
                .title(" Sensitive configuration ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center),
        popup,
    );
}

fn render_edit_confirmation(frame: &mut Frame<'_>, area: Rect, server_name: &str) {
    let popup = centered_rect(area, 78, 9);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(format!(
            "Editing '{server_name}' loads its command, arguments, URL, and environment-variable names into visible fields. These may contain sensitive data. Continue? (y/N)"
        ))
        .block(
            Block::default()
                .title(" Sensitive configuration ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true })
        .alignment(Alignment::Center),
        popup,
    );
}

fn render_notice(frame: &mut Frame<'_>, area: Rect, notice: &str) {
    let popup = centered_rect(area, 76, 7);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(notice)
            .block(Block::default().title(" Message ").borders(Borders::ALL))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center),
        popup,
    );
}

fn centered_rect(area: Rect, width_percent: u16, height: u16) -> Rect {
    let width = area.width.saturating_mul(width_percent).saturating_div(100);
    let width = width.max(20).min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}
