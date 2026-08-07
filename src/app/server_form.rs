use std::collections::{BTreeMap, BTreeSet};

use super::text_edit;
use super::wizard::{format_shell_words, nonempty, parse_env_vars, parse_shell_words};
use super::*;
use crate::config::{
    EditableServer, McpServerOptions, ServerDefinition, ServerField, ServerUpdate,
    StartupTimeoutSource, ToolApprovalMode,
};

#[derive(Clone)]
pub(super) enum FormOrigin {
    Add(Box<AddWizard>),
    Edit { selected: usize },
}

#[derive(Clone)]
pub(super) struct ServerForm {
    pub(super) editable: EditableServer,
    pub(super) fields: Vec<ServerField>,
    pub(super) selected: usize,
    pub(super) dirty_fields: BTreeSet<ServerField>,
    pub(super) expected_base: Fingerprint,
    pub(super) origin: FormOrigin,
}

#[derive(Clone)]
pub(super) struct FieldInputState {
    pub(super) form: ServerForm,
    pub(super) field: ServerField,
    pub(super) value: String,
    pub(super) cursor: usize,
}

#[derive(Clone)]
pub(super) struct ToolListState {
    pub(super) form: ServerForm,
    pub(super) field: ServerField,
    pub(super) selected: usize,
}

#[derive(Clone)]
pub(super) struct ToolNameInputState {
    pub(super) list: ToolListState,
    pub(super) edit_index: Option<usize>,
    pub(super) value: String,
    pub(super) cursor: usize,
}

impl ServerForm {
    pub(super) fn for_add(wizard: AddWizard) -> Self {
        let definition = match wizard.transport {
            WizardTransport::Stdio => ServerDefinition::Stdio {
                command: String::new(),
                args: Vec::new(),
                cwd: None,
                env_vars: Vec::new(),
            },
            WizardTransport::Http => ServerDefinition::Http {
                url: String::new(),
                bearer_token_env_var: None,
            },
        };
        let editable = EditableServer {
            server: NewServer {
                name: wizard.name.clone(),
                definition,
                options: McpServerOptions::default(),
            },
            locked_fields: BTreeSet::new(),
            startup_timeout_source: StartupTimeoutSource::None,
        };
        Self::new(
            editable,
            wizard.expected_base,
            FormOrigin::Add(Box::new(wizard)),
        )
    }

    pub(super) fn for_edit(
        editable: EditableServer,
        expected_base: Fingerprint,
        selected: usize,
    ) -> Self {
        Self::new(editable, expected_base, FormOrigin::Edit { selected })
    }

    fn new(editable: EditableServer, expected_base: Fingerprint, origin: FormOrigin) -> Self {
        let fields = match editable.server.definition {
            ServerDefinition::Stdio { .. } => vec![
                ServerField::Command,
                ServerField::Arguments,
                ServerField::WorkingDirectory,
                ServerField::EnvironmentVariables,
                ServerField::Required,
                ServerField::StartupTimeout,
                ServerField::ToolTimeout,
                ServerField::DefaultToolsApprovalMode,
                ServerField::EnabledTools,
                ServerField::DisabledTools,
            ],
            ServerDefinition::Http { .. } => vec![
                ServerField::Url,
                ServerField::BearerEnvironmentVariable,
                ServerField::Required,
                ServerField::StartupTimeout,
                ServerField::ToolTimeout,
                ServerField::DefaultToolsApprovalMode,
                ServerField::EnabledTools,
                ServerField::DisabledTools,
            ],
        };
        Self {
            editable,
            fields,
            selected: 0,
            dirty_fields: BTreeSet::new(),
            expected_base,
            origin,
        }
    }

    pub(super) fn selected_field(&self) -> ServerField {
        self.fields[self.selected]
    }

    pub(super) fn is_locked(&self, field: ServerField) -> bool {
        self.editable.locked_fields.contains(&field)
    }
}

impl<'a> App<'a> {
    pub(super) fn handle_server_form_key(
        &mut self,
        key: KeyEvent,
        mut form: ServerForm,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc => {
                self.screen = match form.origin {
                    FormOrigin::Add(_) => Screen::Inventory { selected: 0 },
                    FormOrigin::Edit { selected } => Screen::Inventory { selected },
                };
                return Ok(None);
            }
            KeyCode::Up | KeyCode::Char('k') => form.selected = form.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                form.selected = (form.selected + 1).min(form.fields.len().saturating_sub(1));
            }
            KeyCode::Home => form.selected = 0,
            KeyCode::End => form.selected = form.fields.len().saturating_sub(1),
            KeyCode::Char('s') => return self.save_server_form(form),
            KeyCode::Delete | KeyCode::Backspace => self.clear_form_field(&mut form)?,
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                self.cycle_form_field(&mut form)?;
            }
            KeyCode::Enter => {
                let field = form.selected_field();
                if form.is_locked(field) {
                    self.notice = Some(
                        "This field uses an advanced TOML shape. Inspect TOML to edit it safely."
                            .to_owned(),
                    );
                } else if matches!(
                    field,
                    ServerField::EnabledTools | ServerField::DisabledTools
                ) {
                    self.screen = Screen::ToolList(ToolListState {
                        form,
                        field,
                        selected: 0,
                    });
                    return Ok(None);
                } else if matches!(
                    field,
                    ServerField::Required | ServerField::DefaultToolsApprovalMode
                ) {
                    self.cycle_form_field(&mut form)?;
                } else {
                    let value = field_input_value(&form, field);
                    let cursor = text_edit::end(&value);
                    self.screen = Screen::FieldInput(FieldInputState {
                        form,
                        field,
                        value,
                        cursor,
                    });
                    return Ok(None);
                }
            }
            _ => {}
        }
        self.screen = Screen::ServerForm(form);
        Ok(None)
    }

    fn save_server_form(&mut self, form: ServerForm) -> Result<Option<Outcome>, StoreError> {
        validate_new_server(&form.editable.server)?;
        match form.origin {
            FormOrigin::Add(mut wizard) => {
                wizard.server = Some(form.editable.server);
                wizard.step = AddStep::AssignProfiles;
                self.screen = Screen::AddServer(*wizard);
            }
            FormOrigin::Edit { selected } => {
                let server_name = form.editable.server.name.clone();
                self.store.update_server(
                    &ServerUpdate {
                        editable: form.editable,
                        dirty_fields: form.dirty_fields,
                    },
                    form.expected_base,
                )?;
                self.reload()?;
                let selected = self
                    .catalog
                    .base_servers
                    .iter()
                    .position(|item| item.name == server_name)
                    .unwrap_or(selected);
                self.screen = Screen::Inventory { selected };
                self.notice = Some(format!(
                    "Updated '{server_name}'. Unchanged and advanced TOML fields were preserved."
                ));
            }
        }
        Ok(None)
    }

    fn cycle_form_field(&mut self, form: &mut ServerForm) -> Result<(), StoreError> {
        let field = form.selected_field();
        if form.is_locked(field) {
            self.notice = Some("This advanced field is read-only in the form.".to_owned());
            return Ok(());
        }
        match field {
            ServerField::Required => {
                let value = &mut form.editable.server.options.required;
                *value = match *value {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
            }
            ServerField::DefaultToolsApprovalMode => {
                let value = &mut form.editable.server.options.default_tools_approval_mode;
                *value = match *value {
                    None => Some(ToolApprovalMode::Auto),
                    Some(ToolApprovalMode::Approve) => None,
                    Some(mode) => Some(mode.cycle()),
                };
            }
            _ => return Ok(()),
        }
        form.dirty_fields.insert(field);
        Ok(())
    }

    fn clear_form_field(&mut self, form: &mut ServerForm) -> Result<(), StoreError> {
        let field = form.selected_field();
        if form.is_locked(field) {
            self.notice = Some("This advanced field is read-only in the form.".to_owned());
            return Ok(());
        }
        match (&mut form.editable.server.definition, field) {
            (ServerDefinition::Stdio { .. }, ServerField::Command) => {
                self.notice = Some("Command is required and cannot be unset.".to_owned());
                return Ok(());
            }
            (ServerDefinition::Http { .. }, ServerField::Url) => {
                self.notice = Some("URL is required and cannot be unset.".to_owned());
                return Ok(());
            }
            (ServerDefinition::Stdio { args, .. }, ServerField::Arguments) => args.clear(),
            (ServerDefinition::Stdio { cwd, .. }, ServerField::WorkingDirectory) => *cwd = None,
            (ServerDefinition::Stdio { env_vars, .. }, ServerField::EnvironmentVariables) => {
                env_vars.clear()
            }
            (
                ServerDefinition::Http {
                    bearer_token_env_var,
                    ..
                },
                ServerField::BearerEnvironmentVariable,
            ) => *bearer_token_env_var = None,
            (_, ServerField::Required) => form.editable.server.options.required = None,
            (_, ServerField::StartupTimeout) => {
                form.editable.server.options.startup_timeout_sec = None;
                form.editable.startup_timeout_source = StartupTimeoutSource::None;
            }
            (_, ServerField::ToolTimeout) => form.editable.server.options.tool_timeout_sec = None,
            (_, ServerField::EnabledTools) => form.editable.server.options.enabled_tools = None,
            (_, ServerField::DisabledTools) => form.editable.server.options.disabled_tools = None,
            (_, ServerField::DefaultToolsApprovalMode) => {
                form.editable.server.options.default_tools_approval_mode = None
            }
            _ => return Ok(()),
        }
        form.dirty_fields.insert(field);
        Ok(())
    }

    pub(super) fn handle_field_input_key(
        &mut self,
        key: KeyEvent,
        mut input: FieldInputState,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc => self.screen = Screen::ServerForm(input.form),
            KeyCode::Enter => {
                apply_field_input(&mut input.form, input.field, &input.value)?;
                input.form.dirty_fields.insert(input.field);
                self.screen = Screen::ServerForm(input.form);
            }
            KeyCode::Backspace => {
                text_edit::backspace(&mut input.value, &mut input.cursor);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Delete => {
                text_edit::delete(&mut input.value, input.cursor);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Left => {
                text_edit::move_left(&mut input.cursor);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Right => {
                text_edit::move_right(&input.value, &mut input.cursor);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Home => {
                input.cursor = 0;
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::End => {
                input.cursor = text_edit::end(&input.value);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                text_edit::clear(&mut input.value, &mut input.cursor);
                self.screen = Screen::FieldInput(input);
            }
            KeyCode::Char(character) if !character.is_control() => {
                text_edit::insert(&mut input.value, &mut input.cursor, character);
                self.screen = Screen::FieldInput(input);
            }
            _ => self.screen = Screen::FieldInput(input),
        }
        Ok(None)
    }

    pub(super) fn handle_tool_list_key(
        &mut self,
        key: KeyEvent,
        mut list: ToolListState,
    ) -> Result<Option<Outcome>, StoreError> {
        let values = tool_values(&list.form, list.field);
        match key.code {
            KeyCode::Esc => self.screen = Screen::ServerForm(list.form),
            KeyCode::Up | KeyCode::Char('k') => {
                list.selected = list.selected.saturating_sub(1);
                self.screen = Screen::ToolList(list);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                list.selected = (list.selected + 1).min(values.len().saturating_sub(1));
                self.screen = Screen::ToolList(list);
            }
            KeyCode::Char(' ') => {
                let field = list.field;
                let slot = tool_values_mut(&mut list.form, field);
                *slot = if slot.is_none() {
                    Some(Vec::new())
                } else {
                    None
                };
                list.form.dirty_fields.insert(field);
                list.selected = 0;
                self.screen = Screen::ToolList(list);
            }
            KeyCode::Char('a') => {
                self.screen = Screen::ToolNameInput(ToolNameInputState {
                    list,
                    edit_index: None,
                    value: String::new(),
                    cursor: 0,
                });
            }
            KeyCode::Enter | KeyCode::Char('e') if !values.is_empty() => {
                let value = values[list.selected].clone();
                let cursor = text_edit::end(&value);
                let edit_index = Some(list.selected);
                self.screen = Screen::ToolNameInput(ToolNameInputState {
                    list,
                    edit_index,
                    value,
                    cursor,
                });
            }
            KeyCode::Delete | KeyCode::Char('d') if !values.is_empty() => {
                let field = list.field;
                tool_values_mut(&mut list.form, field)
                    .get_or_insert_with(Vec::new)
                    .remove(list.selected);
                list.form.dirty_fields.insert(field);
                let len = tool_values(&list.form, field).len();
                list.selected = list.selected.min(len.saturating_sub(1));
                self.screen = Screen::ToolList(list);
            }
            _ => self.screen = Screen::ToolList(list),
        }
        Ok(None)
    }

    pub(super) fn handle_tool_name_input_key(
        &mut self,
        key: KeyEvent,
        mut input: ToolNameInputState,
    ) -> Result<Option<Outcome>, StoreError> {
        match key.code {
            KeyCode::Esc => self.screen = Screen::ToolList(input.list),
            KeyCode::Enter => {
                let name = input.value.trim();
                if name.is_empty() {
                    return Err(StoreError::InvalidServer(
                        "tool name cannot be empty".to_owned(),
                    ));
                }
                let field = input.list.field;
                let values =
                    tool_values_mut(&mut input.list.form, field).get_or_insert_with(Vec::new);
                if values
                    .iter()
                    .enumerate()
                    .any(|(index, value)| value == name && Some(index) != input.edit_index)
                {
                    return Err(StoreError::InvalidServer(format!(
                        "tool name '{name}' is already in this list"
                    )));
                }
                if let Some(index) = input.edit_index {
                    values[index] = name.to_owned();
                } else {
                    values.push(name.to_owned());
                    input.list.selected = values.len() - 1;
                }
                input.list.form.dirty_fields.insert(field);
                self.screen = Screen::ToolList(input.list);
            }
            KeyCode::Backspace => {
                text_edit::backspace(&mut input.value, &mut input.cursor);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Delete => {
                text_edit::delete(&mut input.value, input.cursor);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Left => {
                text_edit::move_left(&mut input.cursor);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Right => {
                text_edit::move_right(&input.value, &mut input.cursor);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Home => {
                input.cursor = 0;
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::End => {
                input.cursor = text_edit::end(&input.value);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                text_edit::clear(&mut input.value, &mut input.cursor);
                self.screen = Screen::ToolNameInput(input);
            }
            KeyCode::Char(character) if !character.is_control() => {
                text_edit::insert(&mut input.value, &mut input.cursor, character);
                self.screen = Screen::ToolNameInput(input);
            }
            _ => self.screen = Screen::ToolNameInput(input),
        }
        Ok(None)
    }
}

fn field_input_value(form: &ServerForm, field: ServerField) -> String {
    match (&form.editable.server.definition, field) {
        (ServerDefinition::Stdio { command, .. }, ServerField::Command) => command.clone(),
        (ServerDefinition::Stdio { args, .. }, ServerField::Arguments) => format_shell_words(args),
        (ServerDefinition::Stdio { cwd, .. }, ServerField::WorkingDirectory) => {
            cwd.clone().unwrap_or_default()
        }
        (ServerDefinition::Stdio { env_vars, .. }, ServerField::EnvironmentVariables) => {
            env_vars.join(", ")
        }
        (ServerDefinition::Http { url, .. }, ServerField::Url) => url.clone(),
        (
            ServerDefinition::Http {
                bearer_token_env_var,
                ..
            },
            ServerField::BearerEnvironmentVariable,
        ) => bearer_token_env_var.clone().unwrap_or_default(),
        (_, ServerField::StartupTimeout) => {
            format_number(form.editable.server.options.startup_timeout_sec)
        }
        (_, ServerField::ToolTimeout) => {
            format_number(form.editable.server.options.tool_timeout_sec)
        }
        _ => String::new(),
    }
}

fn format_number(value: Option<f64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn apply_field_input(
    form: &mut ServerForm,
    field: ServerField,
    input: &str,
) -> Result<(), StoreError> {
    let trimmed = input.trim();
    match (&mut form.editable.server.definition, field) {
        (ServerDefinition::Stdio { command, .. }, ServerField::Command) => {
            if trimmed.is_empty() {
                return Err(StoreError::InvalidServer(
                    "stdio command cannot be empty".to_owned(),
                ));
            }
            *command = trimmed.to_owned();
        }
        (ServerDefinition::Stdio { args, .. }, ServerField::Arguments) => {
            *args = parse_shell_words(input)?
        }
        (ServerDefinition::Stdio { cwd, .. }, ServerField::WorkingDirectory) => {
            *cwd = nonempty(trimmed)
        }
        (ServerDefinition::Stdio { env_vars, .. }, ServerField::EnvironmentVariables) => {
            *env_vars = parse_env_vars(input)
        }
        (ServerDefinition::Http { url, .. }, ServerField::Url) => {
            if trimmed.is_empty() {
                return Err(StoreError::InvalidServer(
                    "HTTP URL cannot be empty".to_owned(),
                ));
            }
            *url = trimmed.to_owned();
        }
        (
            ServerDefinition::Http {
                bearer_token_env_var,
                ..
            },
            ServerField::BearerEnvironmentVariable,
        ) => *bearer_token_env_var = nonempty(trimmed),
        (_, ServerField::StartupTimeout) => {
            form.editable.server.options.startup_timeout_sec =
                parse_timeout(trimmed, "startup timeout")?;
            form.editable.startup_timeout_source = if trimmed.is_empty() {
                StartupTimeoutSource::None
            } else {
                StartupTimeoutSource::Seconds
            };
        }
        (_, ServerField::ToolTimeout) => {
            form.editable.server.options.tool_timeout_sec = parse_timeout(trimmed, "tool timeout")?
        }
        _ => {}
    }
    Ok(())
}

fn parse_timeout(input: &str, label: &str) -> Result<Option<f64>, StoreError> {
    if input.is_empty() {
        return Ok(None);
    }
    let value = input.parse::<f64>().map_err(|_| {
        StoreError::InvalidServer(format!("{label} must be a non-negative number of seconds"))
    })?;
    if !value.is_finite() || value < 0.0 {
        return Err(StoreError::InvalidServer(format!(
            "{label} must be a non-negative number of seconds"
        )));
    }
    Ok(Some(value))
}

pub(super) fn tool_values(form: &ServerForm, field: ServerField) -> &[String] {
    match field {
        ServerField::EnabledTools => form.editable.server.options.enabled_tools.as_deref(),
        ServerField::DisabledTools => form.editable.server.options.disabled_tools.as_deref(),
        _ => None,
    }
    .unwrap_or(&[])
}

fn tool_values_mut(form: &mut ServerForm, field: ServerField) -> &mut Option<Vec<String>> {
    match field {
        ServerField::EnabledTools => &mut form.editable.server.options.enabled_tools,
        ServerField::DisabledTools => &mut form.editable.server.options.disabled_tools,
        _ => unreachable!(),
    }
}

pub(super) fn form_field_values(form: &ServerForm) -> BTreeMap<ServerField, String> {
    let mut values = BTreeMap::new();
    for field in &form.fields {
        let value = if form.is_locked(*field) {
            "<advanced; inspect TOML>".to_owned()
        } else {
            match (&form.editable.server.definition, field) {
                (ServerDefinition::Stdio { command, .. }, ServerField::Command) => command.clone(),
                (ServerDefinition::Stdio { args, .. }, ServerField::Arguments) => {
                    if args.is_empty() {
                        "<empty>".to_owned()
                    } else {
                        format_shell_words(args)
                    }
                }
                (ServerDefinition::Stdio { cwd, .. }, ServerField::WorkingDirectory) => {
                    cwd.clone().unwrap_or_else(|| "<unset>".to_owned())
                }
                (ServerDefinition::Stdio { env_vars, .. }, ServerField::EnvironmentVariables) => {
                    if env_vars.is_empty() {
                        "<unset>".to_owned()
                    } else {
                        env_vars.join(", ")
                    }
                }
                (ServerDefinition::Http { url, .. }, ServerField::Url) => url.clone(),
                (
                    ServerDefinition::Http {
                        bearer_token_env_var,
                        ..
                    },
                    ServerField::BearerEnvironmentVariable,
                ) => bearer_token_env_var
                    .clone()
                    .unwrap_or_else(|| "<unset>".to_owned()),
                (_, ServerField::Required) => form
                    .editable
                    .server
                    .options
                    .required
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<unset>".to_owned()),
                (_, ServerField::StartupTimeout) => form
                    .editable
                    .server
                    .options
                    .startup_timeout_sec
                    .map(|value| {
                        if form.editable.startup_timeout_source
                            == StartupTimeoutSource::Milliseconds
                            && !form.dirty_fields.contains(&ServerField::StartupTimeout)
                        {
                            format!("{value} sec (stored as ms; edit to convert)")
                        } else {
                            format!("{value} sec")
                        }
                    })
                    .unwrap_or_else(|| "<unset>".to_owned()),
                (_, ServerField::ToolTimeout) => form
                    .editable
                    .server
                    .options
                    .tool_timeout_sec
                    .map(|value| format!("{value} sec"))
                    .unwrap_or_else(|| "<unset>".to_owned()),
                (_, ServerField::DefaultToolsApprovalMode) => form
                    .editable
                    .server
                    .options
                    .default_tools_approval_mode
                    .map(|value| value.as_str().to_owned())
                    .unwrap_or_else(|| "<unset>".to_owned()),
                (_, ServerField::EnabledTools) => {
                    format_tool_list(&form.editable.server.options.enabled_tools)
                }
                (_, ServerField::DisabledTools) => {
                    format_tool_list(&form.editable.server.options.disabled_tools)
                }
                _ => String::new(),
            }
        };
        values.insert(*field, value);
    }
    values
}

fn format_tool_list(values: &Option<Vec<String>>) -> String {
    match values {
        None => "<unset>".to_owned(),
        Some(values) if values.is_empty() => "[] (explicit empty)".to_owned(),
        Some(values) => values.join(", "),
    }
}
