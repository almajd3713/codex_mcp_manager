use std::ffi::OsStr;
use std::fs;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::tempdir;

use super::wizard::{format_shell_words, parse_shell_words};
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
fn formatted_shell_words_round_trip_exactly() {
    let words = vec![
        "--flag".to_owned(),
        "two words".to_owned(),
        String::new(),
        "it's-safe".to_owned(),
        r#"slash\value"#.to_owned(),
    ];
    assert_eq!(
        parse_shell_words(&format_shell_words(&words)).unwrap(),
        words
    );
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
    assert!(matches!(app.screen, Screen::ServerForm(_)));
    app.handle_key(key(KeyCode::Enter)).unwrap();
    for character in "npx".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    for character in "-y @example/mcp".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Char('s'))).unwrap();
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

#[test]
fn edit_server_flow_requires_confirmation_and_updates_prefilled_fields() {
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("config.toml"),
        concat!(
            "[mcp_servers.local]\n",
            "command = \"old-command\"\n",
            "args = [\"--old\"]\n",
            "cwd = \"/old\"\n",
            "env_vars = [\"OLD_TOKEN\"]\n",
            "enabled = false\n",
            "startup_timeout_sec = 30\n",
        ),
    )
    .unwrap();
    let store = ConfigStore::new(temp.path().to_owned());
    let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

    app.handle_key(key(KeyCode::Char('m'))).unwrap();
    app.handle_key(key(KeyCode::Char('e'))).unwrap();
    assert!(matches!(app.screen, Screen::ConfirmEdit { .. }));
    app.handle_key(key(KeyCode::Char('y'))).unwrap();
    let Screen::ServerForm(form) = &app.screen else {
        panic!("expected server form");
    };
    assert_eq!(
        form.editable.server.definition,
        crate::config::ServerDefinition::Stdio {
            command: "old-command".to_owned(),
            args: vec!["--old".to_owned()],
            cwd: Some("/old".to_owned()),
            env_vars: vec!["OLD_TOKEN".to_owned()],
        }
    );

    let clear = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(clear).unwrap();
    for character in "new-command".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(clear).unwrap();
    for character in r#""two words" --new"#.chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(clear).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(clear).unwrap();
    for character in "NEW_TOKEN".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Char('s'))).unwrap();

    assert!(matches!(app.screen, Screen::Inventory { .. }));
    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("command = \"new-command\""));
    assert!(rendered.contains("args = [\"two words\", \"--new\"]"));
    assert!(rendered.contains("env_vars = [\"NEW_TOKEN\"]"));
    assert!(!rendered.contains("cwd ="));
    assert!(rendered.contains("enabled = false"));
    assert!(rendered.contains("startup_timeout_sec = 30"));
}

#[test]
fn server_form_edits_runtime_and_tool_policy_options() {
    let temp = tempdir().unwrap();
    fs::write(
        temp.path().join("config.toml"),
        "[mcp_servers.local]\ncommand = \"server\"\n",
    )
    .unwrap();
    let store = ConfigStore::new(temp.path().to_owned());
    let mut app = App::new(&store, OsStr::new("codex"), None).unwrap();

    app.handle_key(key(KeyCode::Char('m'))).unwrap();
    app.handle_key(key(KeyCode::Char('e'))).unwrap();
    app.handle_key(key(KeyCode::Char('y'))).unwrap();

    for _ in 0..4 {
        app.handle_key(key(KeyCode::Down)).unwrap();
    }
    app.handle_key(key(KeyCode::Char(' '))).unwrap();

    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    for character in "12.5".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();

    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    for character in "90".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();

    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Char(' '))).unwrap();

    app.handle_key(key(KeyCode::Down)).unwrap();
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Char('a'))).unwrap();
    for character in "search".chars() {
        app.handle_key(key(KeyCode::Char(character))).unwrap();
    }
    app.handle_key(key(KeyCode::Enter)).unwrap();
    app.handle_key(key(KeyCode::Esc)).unwrap();
    app.handle_key(key(KeyCode::Char('s'))).unwrap();

    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("required = true"));
    assert!(rendered.contains("startup_timeout_sec = 12.5"));
    assert!(rendered.contains("tool_timeout_sec = 90.0"));
    assert!(rendered.contains("default_tools_approval_mode = \"auto\""));
    assert!(rendered.contains("enabled_tools = [\"search\"]"));
}
