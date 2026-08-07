use super::*;

impl<'a> App<'a> {
    pub(super) fn handle_inventory_key(
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
            KeyCode::Char('e') => {
                if let Some(server) = self.catalog.base_servers.get(selected) {
                    self.screen = Screen::ConfirmEdit {
                        server_name: server.name.clone(),
                        selected,
                    };
                } else {
                    self.screen = Screen::Inventory { selected };
                }
            }
            KeyCode::Char('v') => {
                self.validate_with_codex();
                self.screen = Screen::Inventory { selected };
            }
            KeyCode::Char('i') => {
                if let Some(server) = self.catalog.base_servers.get(selected) {
                    self.screen = Screen::ConfirmInspect {
                        target: InspectTarget::Server(server.name.clone()),
                        back: InspectBack::Inventory { selected },
                    };
                } else {
                    self.screen = Screen::Inventory { selected };
                }
            }
            KeyCode::Char('?') => {
                self.notice = Some(
                    "n add server • e edit server • i inspect TOML • v validate • Esc return to profiles"
                        .to_owned(),
                );
                self.screen = Screen::Inventory { selected };
            }
            _ => self.screen = Screen::Inventory { selected },
        }
        Ok(None)
    }

    pub(super) fn handle_inspect_confirmation(
        &mut self,
        key: KeyEvent,
        target: InspectTarget,
        back: InspectBack,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let (title, source) = match &target {
                    InspectTarget::Base => (
                        "Base — config.toml".to_owned(),
                        self.store.inspect_base_source()?,
                    ),
                    InspectTarget::Profile(name) => (
                        format!("Profile '{name}' — {name}.config.toml"),
                        self.store.inspect_profile_source(name)?,
                    ),
                    InspectTarget::Server(name) => (
                        format!("MCP server '{name}' — native TOML"),
                        self.store.inspect_server_source(name)?,
                    ),
                };
                self.screen = Screen::Inspector(InspectorState {
                    title,
                    source,
                    vertical_scroll: 0,
                    horizontal_scroll: 0,
                    back,
                });
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.restore_inspect_back(back);
            }
            _ => {
                self.screen = Screen::ConfirmInspect { target, back };
            }
        }
        Ok(None)
    }

    pub(super) fn handle_inspector_key(
        &mut self,
        key: KeyEvent,
        mut inspector: InspectorState,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.restore_inspect_back(inspector.back);
                return Ok(None);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                inspector.vertical_scroll = inspector.vertical_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                inspector.vertical_scroll = inspector.vertical_scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                inspector.vertical_scroll = inspector.vertical_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                inspector.vertical_scroll = inspector.vertical_scroll.saturating_add(10);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                inspector.horizontal_scroll = inspector.horizontal_scroll.saturating_sub(4);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                inspector.horizontal_scroll = inspector.horizontal_scroll.saturating_add(4);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                inspector.vertical_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                inspector.vertical_scroll =
                    inspector.source.lines().count().saturating_sub(1) as u16;
            }
            _ => {}
        }
        self.screen = Screen::Inspector(inspector);
        Ok(None)
    }

    fn restore_inspect_back(&mut self, back: InspectBack) {
        self.screen = match back {
            InspectBack::Profiles => Screen::Profiles,
            InspectBack::Inventory { selected } => Screen::Inventory { selected },
        };
    }

    pub(super) fn handle_add_server_key(
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
                    self.screen = Screen::ServerForm(ServerForm::for_add(wizard));
                    return Ok(None);
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
                    let server = wizard.definition()?.clone();
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
            KeyCode::Enter => {
                validate_server_name(wizard.input.trim())?;
                wizard.name = wizard.input.trim().to_owned();
                wizard.input.clear();
                wizard.step = AddStep::Transport;
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
        self.screen = Screen::AddServer(wizard);
        Ok(None)
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
}
