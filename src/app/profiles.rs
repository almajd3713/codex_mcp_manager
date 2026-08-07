use super::text_edit;
use super::*;

impl<'a> App<'a> {
    pub(super) fn handle_profiles_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<Outcome>, StoreError> {
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
                    cursor: 0,
                    action: InputAction::Create,
                });
            }
            KeyCode::Char('e') => self.begin_edit_selected(),
            KeyCode::Char('r') => {
                if let Some(profile) = self.selected_profile() {
                    self.screen = Screen::Input(InputState {
                        title: " Rename profile ",
                        value: profile.name.clone(),
                        cursor: text_edit::end(&profile.name),
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
            KeyCode::Char('i') => {
                let target = if self.selected == 0 {
                    InspectTarget::Base
                } else {
                    InspectTarget::Profile(self.catalog.profiles[self.selected - 1].name.clone())
                };
                self.screen = Screen::ConfirmInspect {
                    target,
                    back: InspectBack::Profiles,
                };
            }
            KeyCode::Char('?') => {
                self.notice = Some(
                    "Enter launch • n new profile • e edit • i inspect TOML • m MCP inventory"
                        .to_owned(),
                );
            }
            _ => {}
        }
        Ok(None)
    }

    pub(super) fn handle_editor_key(
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

    pub(super) fn handle_input_key(
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
                text_edit::backspace(&mut input.value, &mut input.cursor);
                self.screen = Screen::Input(input);
            }
            KeyCode::Delete => {
                text_edit::delete(&mut input.value, input.cursor);
                self.screen = Screen::Input(input);
            }
            KeyCode::Left => {
                text_edit::move_left(&mut input.cursor);
                self.screen = Screen::Input(input);
            }
            KeyCode::Right => {
                text_edit::move_right(&input.value, &mut input.cursor);
                self.screen = Screen::Input(input);
            }
            KeyCode::Home => {
                input.cursor = 0;
                self.screen = Screen::Input(input);
            }
            KeyCode::End => {
                input.cursor = text_edit::end(&input.value);
                self.screen = Screen::Input(input);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                text_edit::clear(&mut input.value, &mut input.cursor);
                self.screen = Screen::Input(input);
            }
            KeyCode::Char(character)
                if character.is_ascii_alphanumeric() || character == '-' || character == '_' =>
            {
                text_edit::insert(&mut input.value, &mut input.cursor, character);
                self.screen = Screen::Input(input);
            }
            _ => self.screen = Screen::Input(input),
        }
        Ok(None)
    }

    pub(super) fn handle_delete_key(
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
