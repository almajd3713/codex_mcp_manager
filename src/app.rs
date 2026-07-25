use std::collections::BTreeMap;
use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use thiserror::Error;

use crate::config::{
    Catalog, ConfigStore, Fingerprint, OverrideState, ProfileRecord, ServerOrigin, ServerView,
    StoreError,
};
use crate::launcher::LaunchTarget;

const TICK_RATE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum Outcome {
    Quit,
    Launch(LaunchTarget),
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("terminal error: {0}")]
    Terminal(#[from] io::Error),
}

#[derive(Clone)]
struct EditorState {
    profile_name: String,
    fingerprint: Fingerprint,
    servers: Vec<ServerView>,
    selected: usize,
}

#[derive(Clone)]
enum InputAction {
    Create,
    Rename { old_name: String },
}

#[derive(Clone)]
struct InputState {
    title: &'static str,
    value: String,
    action: InputAction,
}

#[derive(Clone)]
enum Screen {
    Profiles,
    Editor(EditorState),
    Input(InputState),
    ConfirmDelete { profile_name: String },
}

struct App<'a> {
    store: &'a ConfigStore,
    catalog: Catalog,
    screen: Screen,
    selected: usize,
    notice: Option<String>,
}

impl<'a> App<'a> {
    fn new(store: &'a ConfigStore, notice: Option<String>) -> Result<Self, StoreError> {
        Ok(Self {
            store,
            catalog: store.load_catalog()?,
            screen: Screen::Profiles,
            selected: 0,
            notice,
        })
    }

    fn reload(&mut self) -> Result<(), StoreError> {
        self.catalog = self.store.load_catalog()?;
        self.selected = self.selected.min(self.catalog.profiles.len());
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<Outcome>, StoreError> {
        if key.kind != KeyEventKind::Press {
            return Ok(None);
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(Some(Outcome::Quit));
        }
        if self.notice.take().is_some() {
            return Ok(None);
        }

        match self.screen.clone() {
            Screen::Profiles => self.handle_profiles_key(key),
            Screen::Editor(editor) => self.handle_editor_key(key, editor),
            Screen::Input(input) => self.handle_input_key(key, input),
            Screen::ConfirmDelete { profile_name } => self.handle_delete_key(key, &profile_name),
        }
    }

    fn handle_profiles_key(&mut self, key: KeyEvent) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(Some(Outcome::Quit)),
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.catalog.profiles.len());
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.catalog.profiles.len(),
            KeyCode::Enter => {
                if self.selected == 0 {
                    return Ok(Some(Outcome::Launch(LaunchTarget::Base)));
                }
                let profile = &self.catalog.profiles[self.selected - 1];
                if let Some(error) = &profile.error {
                    self.notice = Some(error.clone());
                } else {
                    return Ok(Some(Outcome::Launch(LaunchTarget::Profile(
                        profile.name.clone(),
                    ))));
                }
            }
            KeyCode::Char('n') => {
                self.screen = Screen::Input(InputState {
                    title: " New profile ",
                    value: String::new(),
                    action: InputAction::Create,
                });
            }
            KeyCode::Char('e') => self.begin_edit_selected(),
            KeyCode::Char('r') => {
                if let Some(profile) = self.selected_profile() {
                    self.screen = Screen::Input(InputState {
                        title: " Rename profile ",
                        value: profile.name.clone(),
                        action: InputAction::Rename {
                            old_name: profile.name.clone(),
                        },
                    });
                }
            }
            KeyCode::Char('d') => {
                if let Some(profile) = self.selected_profile() {
                    self.screen = Screen::ConfirmDelete {
                        profile_name: profile.name.clone(),
                    };
                }
            }
            KeyCode::Char('?') => {
                self.notice =
                    Some("Enter launch • n new • e edit • r rename • d delete • q quit".to_owned());
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_editor_key(
        &mut self,
        key: KeyEvent,
        mut editor: EditorState,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Profiles;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                editor.selected = editor.selected.saturating_sub(1);
                self.screen = Screen::Editor(editor);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                editor.selected = (editor.selected + 1).min(editor.servers.len().saturating_sub(1));
                self.screen = Screen::Editor(editor);
            }
            KeyCode::Home => {
                editor.selected = 0;
                self.screen = Screen::Editor(editor);
            }
            KeyCode::End => {
                editor.selected = editor.servers.len().saturating_sub(1);
                self.screen = Screen::Editor(editor);
            }
            KeyCode::Char(' ') => {
                if let Some(server) = editor.servers.get_mut(editor.selected) {
                    server.state = server.state.cycle();
                    server.effective_enabled = match server.state {
                        OverrideState::Inherited => server.inherited_enabled,
                        OverrideState::Enabled => true,
                        OverrideState::Disabled => false,
                    };
                }
                self.screen = Screen::Editor(editor);
            }
            KeyCode::Char('s') => {
                let states = editor
                    .servers
                    .iter()
                    .map(|server| (server.name.clone(), server.state))
                    .collect::<BTreeMap<_, _>>();
                self.store.save_profile_states(
                    &editor.profile_name,
                    editor.fingerprint,
                    &states,
                )?;
                self.reload()?;
                self.screen = Screen::Profiles;
                self.notice = Some(format!("Saved profile '{}'.", editor.profile_name));
            }
            _ => {
                self.screen = Screen::Editor(editor);
            }
        }
        Ok(None)
    }

    fn handle_input_key(
        &mut self,
        key: KeyEvent,
        mut input: InputState,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc => self.screen = Screen::Profiles,
            KeyCode::Enter => {
                let value = input.value.clone();
                match &input.action {
                    InputAction::Create => {
                        self.store.create_profile(&value)?;
                        self.reload()?;
                        if let Some(index) = self
                            .catalog
                            .profiles
                            .iter()
                            .position(|profile| profile.name == value)
                        {
                            self.selected = index + 1;
                            self.begin_edit_selected();
                        } else {
                            self.screen = Screen::Profiles;
                        }
                    }
                    InputAction::Rename { old_name } => {
                        self.store.rename_profile(old_name, &value)?;
                        self.reload()?;
                        if let Some(index) = self
                            .catalog
                            .profiles
                            .iter()
                            .position(|profile| profile.name == value)
                        {
                            self.selected = index + 1;
                        }
                        self.screen = Screen::Profiles;
                        self.notice = Some(format!("Renamed profile to '{value}'."));
                    }
                }
            }
            KeyCode::Backspace => {
                input.value.pop();
                self.screen = Screen::Input(input);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.value.clear();
                self.screen = Screen::Input(input);
            }
            KeyCode::Char(character)
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' =>
            {
                input.value.push(character);
                self.screen = Screen::Input(input);
            }
            _ => self.screen = Screen::Input(input),
        }
        Ok(None)
    }

    fn handle_delete_key(
        &mut self,
        key: KeyEvent,
        profile_name: &str,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let destination = self.store.trash_profile(profile_name)?;
                self.reload()?;
                self.screen = Screen::Profiles;
                self.notice = Some(format!(
                    "Moved '{profile_name}' to {}.",
                    destination.display()
                ));
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = Screen::Profiles;
            }
            _ => {}
        }
        Ok(None)
    }

    fn selected_profile(&self) -> Option<&ProfileRecord> {
        self.selected
            .checked_sub(1)
            .and_then(|index| self.catalog.profiles.get(index))
    }

    fn begin_edit_selected(&mut self) {
        let Some(profile) = self.selected_profile() else {
            return;
        };
        if let Some(error) = &profile.error {
            self.notice = Some(error.clone());
            return;
        }
        self.screen = Screen::Editor(EditorState {
            profile_name: profile.name.clone(),
            fingerprint: profile.fingerprint,
            servers: profile.servers.clone(),
            selected: 0,
        });
    }
}

pub fn run(store: &ConfigStore, initial_notice: Option<String>) -> Result<Outcome, AppError> {
    let mut tui = Tui::new()?;
    let mut app = App::new(store, initial_notice)?;

    loop {
        tui.terminal.draw(|frame| render(frame, &app))?;
        if event::poll(TICK_RATE)?
            && let Event::Key(key) = event::read()?
        {
            match app.handle_key(key) {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => {}
                Err(error) => {
                    if matches!(error, StoreError::ConcurrentChange(_)) {
                        app.screen = Screen::Profiles;
                        let _ = app.reload();
                    }
                    app.notice = Some(error.to_string());
                }
            }
        }
    }
}

struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

fn render(frame: &mut Frame<'_>, app: &App<'_>) {
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
        Screen::Editor(editor) => render_editor(frame, chunks[1], editor),
        _ => render_profiles(frame, chunks[1], app),
    }

    let footer = match &app.screen {
        Screen::Profiles => "↑/↓ navigate  Enter launch  n new  e edit  r rename  d delete  ? help",
        Screen::Editor(_) => "↑/↓ navigate  Space cycle state  s save  Esc cancel",
        Screen::Input(_) => "Enter confirm  Ctrl-U clear  Esc cancel",
        Screen::ConfirmDelete { .. } => "y move to trash  n/Esc cancel",
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[2],
    );

    match &app.screen {
        Screen::Input(input) => render_input(frame, area, input),
        Screen::ConfirmDelete { profile_name } => render_confirmation(frame, area, profile_name),
        _ => {}
    }
    if let Some(notice) = &app.notice {
        render_notice(frame, area, notice);
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
    let popup = centered_rect(area, 60, 5);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(input.value.as_str())
            .block(Block::default().title(input.title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        popup,
    );
    let cursor_x = popup
        .x
        .saturating_add(1)
        .saturating_add(input.value.len() as u16)
        .min(popup.right().saturating_sub(2));
    frame.set_cursor_position((cursor_x, popup.y.saturating_add(1)));
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

#[cfg(test)]
mod tests {
    use std::fs;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    use super::{App, Outcome, Screen};
    use crate::config::ConfigStore;
    use crate::launcher::LaunchTarget;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn base_launch_is_always_first() {
        let temp = tempdir().unwrap();
        let store = ConfigStore::new(temp.path().to_owned());
        let mut app = App::new(&store, None).unwrap();

        let outcome = app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(outcome, Some(Outcome::Launch(LaunchTarget::Base))));
    }

    #[test]
    fn editor_cycles_inherited_enabled_disabled() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("config.toml"),
            "[mcp_servers.docs]\nurl = \"https://example.test\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("work.config.toml"), "").unwrap();
        let store = ConfigStore::new(temp.path().to_owned());
        let mut app = App::new(&store, None).unwrap();
        app.selected = 1;
        app.handle_key(key(KeyCode::Char('e'))).unwrap();

        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        let Screen::Editor(editor) = &app.screen else {
            panic!("expected editor");
        };
        assert_eq!(
            editor.servers[0].state,
            crate::config::OverrideState::Enabled
        );

        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        let Screen::Editor(editor) = &app.screen else {
            panic!("expected editor");
        };
        assert_eq!(
            editor.servers[0].state,
            crate::config::OverrideState::Disabled
        );
    }

    #[test]
    fn create_edit_and_save_flow_writes_only_the_override() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("config.toml"),
            concat!(
                "[mcp_servers.docs]\n",
                "url = \"https://example.test?token=SECRET\"\n"
            ),
        )
        .unwrap();
        let store = ConfigStore::new(temp.path().to_owned());
        let mut app = App::new(&store, None).unwrap();

        app.handle_key(key(KeyCode::Char('n'))).unwrap();
        for character in "work".chars() {
            app.handle_key(key(KeyCode::Char(character))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(matches!(app.screen, Screen::Editor(_)));

        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        app.handle_key(key(KeyCode::Char('s'))).unwrap();

        let rendered = fs::read_to_string(temp.path().join("work.config.toml")).unwrap();
        assert!(rendered.contains("[mcp_servers.docs]"));
        assert!(rendered.contains("enabled = true"));
        assert!(!rendered.contains("SECRET"));
    }
}
