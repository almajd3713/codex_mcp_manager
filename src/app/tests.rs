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

#[test]
fn inspection_requires_confirmation_and_returns_to_origin() {
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("config.toml"),
        "sensitive_value = \"VISIBLE_AFTER_CONFIRMATION\"\n",
    )
    .unwrap();
    let store = ConfigStore::new(temp.path().to_owned());
    let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

    app.handle_key(key(KeyCode::Char('i'))).unwrap();
    assert!(matches!(app.screen, Screen::ConfirmInspect { .. }));

    app.handle_key(key(KeyCode::Char('y'))).unwrap();
    let Screen::Inspector(inspector) = &app.screen else {
        panic!("expected inspector");
    };
    assert!(inspector.source.contains("VISIBLE_AFTER_CONFIRMATION"));

    app.handle_key(key(KeyCode::Esc)).unwrap();
    assert!(matches!(app.screen, Screen::Profiles));
}
