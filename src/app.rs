use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Stdout};
use std::process::{Command, Stdio};
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
    Catalog, ConfigStore, Fingerprint, NewServer, OverrideState, ProfileRecord, ServerDefinition,
    ServerOrigin, ServerView, StoreError, TransportKind, validate_new_server, validate_server_name,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WizardTransport {
    Stdio,
    Http,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AddStep {
    Name,
    Transport,
    Command,
    Arguments,
    WorkingDirectory,
    EnvironmentVariables,
    Url,
    BearerEnvironmentVariable,
    AssignProfiles,
}

#[derive(Clone)]
struct ProfileChoice {
    name: String,
    fingerprint: Fingerprint,
    selected: bool,
    available: bool,
}

#[derive(Clone)]
struct AddWizard {
    step: AddStep,
    input: String,
    name: String,
    transport: WizardTransport,
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    env_vars: Vec<String>,
    url: String,
    bearer_token_env_var: Option<String>,
    choices: Vec<ProfileChoice>,
    selected_choice: usize,
    expected_base: Fingerprint,
}

impl AddWizard {
    fn new(catalog: &Catalog) -> Self {
        Self {
            step: AddStep::Name,
            input: String::new(),
            name: String::new(),
            transport: WizardTransport::Stdio,
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env_vars: Vec::new(),
            url: String::new(),
            bearer_token_env_var: None,
            choices: catalog
                .profiles
                .iter()
                .map(|profile| ProfileChoice {
                    name: profile.name.clone(),
                    fingerprint: profile.fingerprint,
                    selected: false,
                    available: profile.error.is_none(),
                })
                .collect(),
            selected_choice: 0,
            expected_base: catalog.base_fingerprint,
        }
    }

    fn definition(&self) -> NewServer {
        let definition = match self.transport {
            WizardTransport::Stdio => ServerDefinition::Stdio {
                command: self.command.clone(),
                args: self.args.clone(),
                cwd: self.cwd.clone(),
                env_vars: self.env_vars.clone(),
            },
            WizardTransport::Http => ServerDefinition::Http {
                url: self.url.clone(),
                bearer_token_env_var: self.bearer_token_env_var.clone(),
            },
        };
        NewServer {
            name: self.name.clone(),
            definition,
        }
    }
}

#[derive(Clone)]
enum Screen {
    Profiles,
    Editor(EditorState),
    Inventory { selected: usize },
    AddServer(AddWizard),
    Input(InputState),
    ConfirmDelete { profile_name: String },
}

struct App<'a> {
    store: &'a ConfigStore,
    codex_bin: &'a OsStr,
    catalog: Catalog,
    screen: Screen,
    selected: usize,
    notice: Option<String>,
}

impl<'a> App<'a> {
    fn new(
        store: &'a ConfigStore,
        codex_bin: &'a OsStr,
        notice: Option<String>,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            store,
            codex_bin,
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
            Screen::Inventory { selected } => self.handle_inventory_key(key, selected),
            Screen::AddServer(wizard) => self.handle_add_server_key(key, wizard),
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
            KeyCode::Char('m') => {
                self.screen = Screen::Inventory { selected: 0 };
            }
            KeyCode::Char('?') => {
                self.notice = Some(
                    "Enter launch • n new profile • e edit • m MCP inventory • r rename • d delete"
                        .to_owned(),
                );
            }
            _ => {}
        }
        Ok(None)
    }

    fn handle_inventory_key(
        &mut self,
        key: KeyEvent,
        mut selected: usize,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Profiles,
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
                self.screen = Screen::Inventory { selected };
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(self.catalog.base_servers.len().saturating_sub(1));
                self.screen = Screen::Inventory { selected };
            }
            KeyCode::Home => self.screen = Screen::Inventory { selected: 0 },
            KeyCode::End => {
                self.screen = Screen::Inventory {
                    selected: self.catalog.base_servers.len().saturating_sub(1),
                };
            }
            KeyCode::Char('n') => {
                self.screen = Screen::AddServer(AddWizard::new(&self.catalog));
            }
            KeyCode::Char('v') => {
                self.validate_with_codex();
                self.screen = Screen::Inventory { selected };
            }
            KeyCode::Char('?') => {
                self.notice = Some(
                    "n add server • v validate with Codex • Esc return to profiles".to_owned(),
                );
                self.screen = Screen::Inventory { selected };
            }
            _ => self.screen = Screen::Inventory { selected },
        }
        Ok(None)
    }

    fn handle_add_server_key(
        &mut self,
        key: KeyEvent,
        mut wizard: AddWizard,
    ) -> Result<Option<Outcome>, StoreError> {
        if key.code == KeyCode::Esc {
            self.screen = Screen::Inventory { selected: 0 };
            return Ok(None);
        }

        if wizard.step == AddStep::Transport {
            match key.code {
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('j')
                | KeyCode::Char('k')
                | KeyCode::Char(' ') => {
                    wizard.transport = match wizard.transport {
                        WizardTransport::Stdio => WizardTransport::Http,
                        WizardTransport::Http => WizardTransport::Stdio,
                    };
                }
                KeyCode::Char('s') => wizard.transport = WizardTransport::Stdio,
                KeyCode::Char('h') => wizard.transport = WizardTransport::Http,
                KeyCode::Enter => {
                    wizard.step = match wizard.transport {
                        WizardTransport::Stdio => AddStep::Command,
                        WizardTransport::Http => AddStep::Url,
                    };
                    wizard.input.clear();
                }
                _ => {}
            }
            self.screen = Screen::AddServer(wizard);
            return Ok(None);
        }

        if wizard.step == AddStep::AssignProfiles {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    wizard.selected_choice = wizard.selected_choice.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    wizard.selected_choice =
                        (wizard.selected_choice + 1).min(wizard.choices.len().saturating_sub(1));
                }
                KeyCode::Char(' ') => {
                    if let Some(choice) = wizard.choices.get_mut(wizard.selected_choice)
                        && choice.available
                    {
                        choice.selected = !choice.selected;
                    }
                }
                KeyCode::Char('s') | KeyCode::Enter => {
                    let server = wizard.definition();
                    validate_new_server(&server)?;
                    let assignments = wizard
                        .choices
                        .iter()
                        .filter(|choice| choice.available && choice.selected)
                        .map(|choice| (choice.name.clone(), choice.fingerprint))
                        .collect::<Vec<_>>();
                    let failed = self.store.add_server_and_assign(
                        &server,
                        wizard.expected_base,
                        &assignments,
                    )?;
                    self.reload()?;
                    let selected = self
                        .catalog
                        .base_servers
                        .iter()
                        .position(|item| item.name == server.name)
                        .unwrap_or(0);
                    self.screen = Screen::Inventory { selected };
                    self.notice = Some(if failed.is_empty() {
                        format!(
                            "Added '{}' disabled in Base and enabled it in {} profile(s).",
                            server.name,
                            assignments.len()
                        )
                    } else {
                        format!(
                            "Added '{}' disabled in Base, but assignment failed for: {}.",
                            server.name,
                            failed.join(", ")
                        )
                    });
                    return Ok(None);
                }
                _ => {}
            }
            self.screen = Screen::AddServer(wizard);
            return Ok(None);
        }

        match key.code {
            KeyCode::Enter => self.advance_add_wizard(&mut wizard)?,
            KeyCode::Backspace => {
                wizard.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                wizard.input.clear();
            }
            KeyCode::Char(character) if !character.is_control() => {
                wizard.input.push(character);
            }
            _ => {}
        }
        self.screen = Screen::AddServer(wizard);
        Ok(None)
    }

    fn advance_add_wizard(&self, wizard: &mut AddWizard) -> Result<(), StoreError> {
        match wizard.step {
            AddStep::Name => {
                validate_server_name(wizard.input.trim())?;
                wizard.name = wizard.input.trim().to_owned();
                wizard.input.clear();
                wizard.step = AddStep::Transport;
            }
            AddStep::Command => {
                if wizard.input.trim().is_empty() {
                    return Err(StoreError::InvalidServer(
                        "stdio command cannot be empty".to_owned(),
                    ));
                }
                wizard.command = wizard.input.trim().to_owned();
                wizard.input.clear();
                wizard.step = AddStep::Arguments;
            }
            AddStep::Arguments => {
                wizard.args = parse_shell_words(&wizard.input)?;
                wizard.input.clear();
                wizard.step = AddStep::WorkingDirectory;
            }
            AddStep::WorkingDirectory => {
                wizard.cwd = nonempty(wizard.input.trim());
                wizard.input.clear();
                wizard.step = AddStep::EnvironmentVariables;
            }
            AddStep::EnvironmentVariables => {
                wizard.env_vars = parse_env_vars(&wizard.input);
                let server = wizard.definition();
                validate_new_server(&server)?;
                wizard.input.clear();
                wizard.step = AddStep::AssignProfiles;
            }
            AddStep::Url => {
                wizard.url = wizard.input.trim().to_owned();
                let server = wizard.definition();
                validate_new_server(&server)?;
                wizard.input.clear();
                wizard.step = AddStep::BearerEnvironmentVariable;
            }
            AddStep::BearerEnvironmentVariable => {
                wizard.bearer_token_env_var = nonempty(wizard.input.trim());
                let server = wizard.definition();
                validate_new_server(&server)?;
                wizard.input.clear();
                wizard.step = AddStep::AssignProfiles;
            }
            AddStep::Transport | AddStep::AssignProfiles => {}
        }
        Ok(())
    }

    fn validate_with_codex(&mut self) {
        let result = Command::new(self.codex_bin)
            .args(["mcp", "list", "--json"])
            .env("CODEX_HOME", self.store.home())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        self.notice = Some(match result {
            Ok(status) if status.success() => {
                "Codex accepted the current MCP configuration.".to_owned()
            }
            Ok(_) => {
                "Codex rejected the current configuration. No values were displayed.".to_owned()
            }
            Err(error) => format!("Could not run Codex validation: {error}"),
        });
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

pub fn run(
    store: &ConfigStore,
    codex_bin: &OsStr,
    initial_notice: Option<String>,
) -> Result<Outcome, AppError> {
    let mut tui = Tui::new()?;
    let mut app = App::new(store, codex_bin, initial_notice)?;

    loop {
        tui.terminal.draw(|frame| render(frame, &app))?;
        if event::poll(TICK_RATE)?
            && let Event::Key(key) = event::read()?
        {
            match app.handle_key(key) {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => {}
                Err(error) => {
                    match &error {
                        StoreError::ConcurrentChange(_) => {
                            app.screen = Screen::Profiles;
                            let _ = app.reload();
                        }
                        StoreError::BaseConcurrentChange => {
                            app.screen = Screen::Inventory { selected: 0 };
                            let _ = app.reload();
                        }
                        _ => {}
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
        Screen::Inventory { selected } => {
            render_inventory(frame, chunks[1], app, *selected);
        }
        Screen::AddServer(wizard) => render_add_server(frame, chunks[1], wizard),
        _ => render_profiles(frame, chunks[1], app),
    }

    let footer = match &app.screen {
        Screen::Profiles => {
            "↑/↓ navigate  Enter launch  n new profile  e edit  m MCP inventory  ? help"
        }
        Screen::Editor(_) => "↑/↓ navigate  Space cycle state  s save  Esc cancel",
        Screen::Inventory { .. } => "↑/↓ navigate  n add server  v validate  Esc profiles",
        Screen::AddServer(wizard) if wizard.step == AddStep::AssignProfiles => {
            "↑/↓ navigate  Space toggle profile  s/Enter save  Esc cancel"
        }
        Screen::AddServer(wizard) if wizard.step == AddStep::Transport => {
            "↑/↓ or s/h choose transport  Enter continue  Esc cancel"
        }
        Screen::AddServer(_) => "Enter continue  Ctrl-U clear  Esc cancel",
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
        AddStep::Transport => render_transport_choice(frame, area, wizard),
        AddStep::AssignProfiles => render_profile_choices(frame, area, wizard),
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
        AddStep::Command => (
            " Local server — executable ",
            "Executable name or absolute path, for example npx.",
        ),
        AddStep::Arguments => (
            " Local server — arguments (optional) ",
            "Shell-style quoting is supported. Do not place secrets here; use environment variables.",
        ),
        AddStep::WorkingDirectory => (
            " Local server — working directory (optional) ",
            "Leave empty to inherit the Codex working directory.",
        ),
        AddStep::EnvironmentVariables => (
            " Local server — inherited environment variables (optional) ",
            "Comma- or space-separated variable names; values are never entered here.",
        ),
        AddStep::Url => (
            " Remote server — HTTP URL ",
            "Must begin with https:// or http://.",
        ),
        AddStep::BearerEnvironmentVariable => (
            " Remote server — bearer-token environment variable (optional) ",
            "Enter a variable name such as MY_MCP_TOKEN, never the token value.",
        ),
        AddStep::Transport | AddStep::AssignProfiles => unreachable!(),
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
    frame.render_widget(
        Paragraph::new(wizard.input.as_str())
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
        .saturating_add(wizard.input.len() as u16)
        .min(chunks[1].right().saturating_sub(2));
    frame.set_cursor_position((cursor_x, chunks[1].y.saturating_add(1)));
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

fn nonempty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn parse_env_vars(input: &str) -> Vec<String> {
    input
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_shell_words(input: &str) -> Result<Vec<String>, StoreError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        match (quote, character) {
            (Quote::None | Quote::Double, '\\') => {
                escaped = true;
                started = true;
            }
            (Quote::None, '\'') => {
                quote = Quote::Single;
                started = true;
            }
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::None, '"') => {
                quote = Quote::Double;
                started = true;
            }
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (_, character) => {
                current.push(character);
                started = true;
            }
        }
    }

    if escaped || quote != Quote::None {
        return Err(StoreError::InvalidServer(
            "arguments contain an unmatched quote or trailing escape".to_owned(),
        ));
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    use super::{App, Outcome, Screen, parse_shell_words};
    use crate::config::ConfigStore;
    use crate::launcher::LaunchTarget;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn base_launch_is_always_first() {
        let temp = tempdir().unwrap();
        let store = ConfigStore::new(temp.path().to_owned());
        let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

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
        let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();
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
        let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

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

    #[test]
    fn shell_words_support_quotes_and_empty_arguments() {
        assert_eq!(
            parse_shell_words(r#"--flag "two words" '' escaped\ value"#).unwrap(),
            ["--flag", "two words", "", "escaped value"]
        );
        assert!(parse_shell_words("'unfinished").is_err());
    }

    #[test]
    fn add_server_wizard_creates_disabled_base_definition_and_profile_override() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("config.toml"), "").unwrap();
        fs::write(temp.path().join("work.config.toml"), "").unwrap();
        let store = ConfigStore::new(temp.path().to_owned());
        let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

        app.handle_key(key(KeyCode::Char('m'))).unwrap();
        app.handle_key(key(KeyCode::Char('n'))).unwrap();
        for character in "docs".chars() {
            app.handle_key(key(KeyCode::Char(character))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        for character in "npx".chars() {
            app.handle_key(key(KeyCode::Char(character))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();
        for character in "-y @example/mcp".chars() {
            app.handle_key(key(KeyCode::Char(character))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();
        app.handle_key(key(KeyCode::Enter)).unwrap();
        for character in "EXAMPLE_TOKEN".chars() {
            app.handle_key(key(KeyCode::Char(character))).unwrap();
        }
        app.handle_key(key(KeyCode::Enter)).unwrap();
        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        app.handle_key(key(KeyCode::Char('s'))).unwrap();

        let base = fs::read_to_string(temp.path().join("config.toml")).unwrap();
        let profile = fs::read_to_string(temp.path().join("work.config.toml")).unwrap();
        assert!(base.contains("[mcp_servers.docs]"));
        assert!(base.contains("enabled = false"));
        assert!(profile.contains("[mcp_servers.docs]"));
        assert!(profile.contains("enabled = true"));
        assert!(matches!(app.screen, Screen::Inventory { .. }));
    }
}
