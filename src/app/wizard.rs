use crate::config::{Catalog, Fingerprint, NewServer, ServerDefinition, StoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WizardTransport {
    Stdio,
    Http,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AddStep {
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
pub(super) struct ProfileChoice {
    pub(super) name: String,
    pub(super) fingerprint: Fingerprint,
    pub(super) selected: bool,
    pub(super) available: bool,
}

#[derive(Clone)]
pub(super) struct AddWizard {
    pub(super) step: AddStep,
    pub(super) input: String,
    pub(super) name: String,
    pub(super) transport: WizardTransport,
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: Option<String>,
    pub(super) env_vars: Vec<String>,
    pub(super) url: String,
    pub(super) bearer_token_env_var: Option<String>,
    pub(super) choices: Vec<ProfileChoice>,
    pub(super) selected_choice: usize,
    pub(super) expected_base: Fingerprint,
}

impl AddWizard {
    pub(super) fn new(catalog: &Catalog) -> Self {
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

    pub(super) fn definition(&self) -> NewServer {
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

pub(super) fn nonempty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

pub(super) fn parse_env_vars(input: &str) -> Vec<String> {
    input
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn parse_shell_words(input: &str) -> Result<Vec<String>, StoreError> {
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
