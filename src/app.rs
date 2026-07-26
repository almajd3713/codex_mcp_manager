use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::process::{Command, Stdio};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use thiserror::Error;

use crate::config::{
    Catalog, ConfigStore, Fingerprint, NewServer, OverrideState, ProfileRecord, ServerView,
    StoreError, validate_new_server, validate_server_name,
};
use crate::launcher::LaunchTarget;

mod inventory;
mod profiles;
mod render;
mod server_editor;
mod terminal;
mod wizard;

#[cfg(test)]
mod tests;

use render::render;
use terminal::Tui;
use wizard::{
    AddStep, AddWizard, EditWizard, WizardTransport, format_shell_words, nonempty, parse_env_vars,
    parse_shell_words,
};

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
enum InspectTarget {
    Base,
    Profile(String),
    Server(String),
}

#[derive(Clone)]
enum InspectBack {
    Profiles,
    Inventory { selected: usize },
}

#[derive(Clone)]
struct InspectorState {
    title: String,
    source: String,
    vertical_scroll: u16,
    horizontal_scroll: u16,
    back: InspectBack,
}

#[derive(Clone)]
enum Screen {
    Profiles,
    Editor(EditorState),
    Inventory {
        selected: usize,
    },
    AddServer(AddWizard),
    EditServer(EditWizard),
    Input(InputState),
    ConfirmDelete {
        profile_name: String,
    },
    ConfirmInspect {
        target: InspectTarget,
        back: InspectBack,
    },
    ConfirmEdit {
        server_name: String,
        selected: usize,
    },
    Inspector(InspectorState),
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
            Screen::EditServer(wizard) => self.handle_edit_server_key(key, wizard),
            Screen::Input(input) => self.handle_input_key(key, input),
            Screen::ConfirmDelete { profile_name } => self.handle_delete_key(key, &profile_name),
            Screen::ConfirmInspect { target, back } => {
                self.handle_inspect_confirmation(key, target, back)
            }
            Screen::ConfirmEdit {
                server_name,
                selected,
            } => self.handle_edit_confirmation(key, &server_name, selected),
            Screen::Inspector(inspector) => self.handle_inspector_key(key, inspector),
        }
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
