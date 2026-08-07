use crate::config::{Catalog, Fingerprint, NewServer, StoreError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WizardTransport {
    Stdio,
    Http,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AddStep {
    Name,
    Transport,
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
    pub(super) server: Option<NewServer>,
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
            server: None,
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

    pub(super) fn definition(&self) -> Result<&NewServer, StoreError> {
        self.server.as_ref().ok_or_else(|| {
            StoreError::InvalidServer("server fields have not been completed".to_owned())
        })
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

pub(super) fn format_shell_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r#"'\''"#)))
        .collect::<Vec<_>>()
        .join(" ")
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
