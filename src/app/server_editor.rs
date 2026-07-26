use super::*;

impl<'a> App<'a> {
    pub(super) fn handle_edit_confirmation(
        &mut self,
        key: KeyEvent,
        server_name: &str,
        selected: usize,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let server = self.store.load_server_definition(server_name)?;
                self.screen = Screen::EditServer(EditWizard::new(
                    server,
                    self.catalog.base_fingerprint,
                    selected,
                ));
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = Screen::Inventory { selected };
            }
            _ => {
                self.screen = Screen::ConfirmEdit {
                    server_name: server_name.to_owned(),
                    selected,
                };
            }
        }
        Ok(None)
    }

    pub(super) fn handle_edit_server_key(
        &mut self,
        key: KeyEvent,
        mut wizard: EditWizard,
    ) -> Result<Option<Outcome>, StoreError> {
        if key.code == KeyCode::Esc {
            self.screen = Screen::Inventory {
                selected: wizard.selected,
            };
            return Ok(None);
        }

        match key.code {
            KeyCode::Enter => {
                if let Some(server) = self.advance_edit_wizard(&mut wizard)? {
                    let server_name = server.name.clone();
                    let selected = wizard.selected;
                    self.store.update_server(&server, wizard.expected_base)?;
                    self.reload()?;
                    let selected = self
                        .catalog
                        .base_servers
                        .iter()
                        .position(|item| item.name == server_name)
                        .unwrap_or(selected);
                    self.screen = Screen::Inventory { selected };
                    self.notice = Some(format!(
                        "Updated '{server_name}'. Base enablement and advanced TOML fields were preserved."
                    ));
                    return Ok(None);
                }
            }
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
        self.screen = Screen::EditServer(wizard);
        Ok(None)
    }

    fn advance_edit_wizard(
        &self,
        wizard: &mut EditWizard,
    ) -> Result<Option<NewServer>, StoreError> {
        match wizard.step {
            AddStep::Command => {
                if wizard.input.trim().is_empty() {
                    return Err(StoreError::InvalidServer(
                        "stdio command cannot be empty".to_owned(),
                    ));
                }
                wizard.command = wizard.input.trim().to_owned();
                wizard.step = AddStep::Arguments;
                wizard.input = format_shell_words(&wizard.args);
            }
            AddStep::Arguments => {
                wizard.args = parse_shell_words(&wizard.input)?;
                wizard.step = AddStep::WorkingDirectory;
                wizard.input = wizard.cwd.clone().unwrap_or_default();
            }
            AddStep::WorkingDirectory => {
                wizard.cwd = nonempty(wizard.input.trim());
                wizard.step = AddStep::EnvironmentVariables;
                wizard.input = wizard.env_vars.join(", ");
            }
            AddStep::EnvironmentVariables => {
                wizard.env_vars = parse_env_vars(&wizard.input);
                let server = wizard.definition();
                validate_new_server(&server)?;
                return Ok(Some(server));
            }
            AddStep::Url => {
                wizard.url = wizard.input.trim().to_owned();
                let server = wizard.definition();
                validate_new_server(&server)?;
                wizard.step = AddStep::BearerEnvironmentVariable;
                wizard.input = wizard.bearer_token_env_var.clone().unwrap_or_default();
            }
            AddStep::BearerEnvironmentVariable => {
                wizard.bearer_token_env_var = nonempty(wizard.input.trim());
                let server = wizard.definition();
                validate_new_server(&server)?;
                return Ok(Some(server));
            }
            AddStep::Name | AddStep::Transport | AddStep::AssignProfiles => {}
        }
        Ok(None)
    }
}
