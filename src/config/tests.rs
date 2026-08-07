use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use tempfile::tempdir;

use super::{
    ConfigStore, McpServerOptions, NewServer, OverrideState, ServerDefinition, ServerField,
    ServerOrigin, ServerUpdate, StartupTimeoutSource, StoreError, ToolApprovalMode, TransportKind,
    validate_new_server,
};

fn store_with_base(base: &str) -> (tempfile::TempDir, ConfigStore) {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("config.toml"), base).unwrap();
    let store = ConfigStore::new(temp.path().to_owned());
    (temp, store)
}

#[test]
fn overlays_profile_states_without_reading_secret_values() {
    let (temp, store) = store_with_base(
        r#"
[mcp_servers.docs]
url = "https://example.test/mcp?token=SECRET"

[mcp_servers.local]
command = "server"
enabled = false
"#,
    );
    fs::write(
        temp.path().join("work.config.toml"),
        r#"
[mcp_servers.docs]
enabled = false

[mcp_servers.local]
enabled = true
"#,
    )
    .unwrap();

    let catalog = store.load_catalog().unwrap();
    let work = &catalog.profiles[0];
    assert_eq!(work.enabled_count(), 1);
    assert_eq!(work.servers[0].state, OverrideState::Disabled);
    assert_eq!(work.servers[1].state, OverrideState::Enabled);
    assert!(
        work.servers
            .iter()
            .all(|server| server.origin == ServerOrigin::Base)
    );
}

#[test]
fn save_preserves_unrelated_content_and_comments() {
    let (temp, store) = store_with_base(
        r#"
[mcp_servers.docs]
url = "https://example.test/mcp"
"#,
    );
    let profile_path = temp.path().join("work.config.toml");
    fs::write(
        &profile_path,
        r#"# keep this comment
model = "some-model"

[mcp_servers.docs]
# keep this server comment
enabled = true
"#,
    )
    .unwrap();
    let profile = store.load_catalog().unwrap().profiles.remove(0);
    let states = BTreeMap::from([("docs".to_owned(), OverrideState::Disabled)]);

    store
        .save_profile_states("work", profile.fingerprint, &states)
        .unwrap();

    let rendered = fs::read_to_string(profile_path).unwrap();
    assert!(rendered.contains("# keep this comment"));
    assert!(rendered.contains("# keep this server comment"));
    assert!(rendered.contains("model = \"some-model\""));
    assert!(rendered.contains("enabled = false"));
}

#[test]
fn inherited_removes_empty_override_table() {
    let (temp, store) = store_with_base(
        r#"
[mcp_servers.docs]
url = "https://example.test/mcp"
"#,
    );
    let profile_path = temp.path().join("work.config.toml");
    fs::write(&profile_path, "[mcp_servers.docs]\nenabled = false\n").unwrap();
    let profile = store.load_catalog().unwrap().profiles.remove(0);
    let states = BTreeMap::from([("docs".to_owned(), OverrideState::Inherited)]);

    store
        .save_profile_states("work", profile.fingerprint, &states)
        .unwrap();

    assert!(!fs::read_to_string(profile_path).unwrap().contains("docs"));
}

#[test]
fn refuses_to_overwrite_external_changes() {
    let (temp, store) = store_with_base("");
    store.create_profile("work").unwrap();
    let profile = store.load_catalog().unwrap().profiles.remove(0);
    fs::write(
        temp.path().join("work.config.toml"),
        "model = \"changed\"\n",
    )
    .unwrap();

    let error = store
        .save_profile_states("work", profile.fingerprint, &BTreeMap::new())
        .unwrap_err();
    assert!(matches!(error, StoreError::ConcurrentChange(_)));
}

#[test]
fn deletion_is_recoverable() {
    let (temp, store) = store_with_base("");
    store.create_profile("work").unwrap();
    let trashed = store.trash_profile("work").unwrap();

    assert!(!temp.path().join("work.config.toml").exists());
    assert!(trashed.exists());
    assert!(trashed.starts_with(temp.path().join(".codex-mcp-trash")));
}

#[test]
fn malformed_toml_does_not_echo_values() {
    let (temp, store) = store_with_base("");
    fs::write(
        temp.path().join("broken.config.toml"),
        "secret = \"DO_NOT_ECHO\n",
    )
    .unwrap();

    let catalog = store.load_catalog().unwrap();
    let error = catalog.profiles[0].error.as_deref().unwrap();
    assert!(!error.contains("DO_NOT_ECHO"));
    assert!(error.contains("could not parse TOML"));
}

#[test]
fn adds_stdio_server_disabled_and_assigns_selected_profiles() {
    let (temp, store) = store_with_base("# preserve base comment\n");
    store.create_profile("work").unwrap();
    store.create_profile("personal").unwrap();
    let catalog = store.load_catalog().unwrap();
    let work = catalog
        .profiles
        .iter()
        .find(|profile| profile.name == "work")
        .unwrap();
    let server = NewServer {
        name: "local-tools".to_owned(),
        definition: ServerDefinition::Stdio {
            command: "npx".to_owned(),
            args: vec!["-y".to_owned(), "@example/mcp".to_owned()],
            cwd: Some("/tmp/work".to_owned()),
            env_vars: vec!["EXAMPLE_TOKEN".to_owned()],
        },
        options: McpServerOptions::default(),
    };

    let failed = store
        .add_server_and_assign(
            &server,
            catalog.base_fingerprint,
            &[("work".to_owned(), work.fingerprint)],
        )
        .unwrap();

    assert!(failed.is_empty());
    let base = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(base.contains("# preserve base comment"));
    assert!(base.contains("[mcp_servers.local-tools]"));
    assert!(base.contains("command = \"npx\""));
    assert!(base.contains("args = [\"-y\", \"@example/mcp\"]"));
    assert!(base.contains("env_vars = [\"EXAMPLE_TOKEN\"]"));
    assert!(base.contains("enabled = false"));

    let updated = store.load_catalog().unwrap();
    assert_eq!(updated.base_servers[0].transport, TransportKind::Stdio);
    assert!(!updated.base_servers[0].effective_enabled);
    let work = updated
        .profiles
        .iter()
        .find(|profile| profile.name == "work")
        .unwrap();
    let personal = updated
        .profiles
        .iter()
        .find(|profile| profile.name == "personal")
        .unwrap();
    assert!(work.servers[0].effective_enabled);
    assert!(!personal.servers[0].effective_enabled);
}

#[test]
fn adds_http_server_with_environment_backed_bearer_token() {
    let (temp, store) = store_with_base("");
    let catalog = store.load_catalog().unwrap();
    let server = NewServer {
        name: "remote".to_owned(),
        definition: ServerDefinition::Http {
            url: "https://mcp.example.test/api".to_owned(),
            bearer_token_env_var: Some("REMOTE_MCP_TOKEN".to_owned()),
        },
        options: McpServerOptions::default(),
    };

    store
        .add_server_and_assign(&server, catalog.base_fingerprint, &[])
        .unwrap();

    let base = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(base.contains("url = \"https://mcp.example.test/api\""));
    assert!(base.contains("bearer_token_env_var = \"REMOTE_MCP_TOKEN\""));
    assert!(!base.contains("token-value"));
    assert_eq!(
        store.load_catalog().unwrap().base_servers[0].transport,
        TransportKind::Http
    );
}

#[test]
fn rejects_duplicate_and_invalid_server_definitions() {
    let (_temp, store) = store_with_base("[mcp_servers.docs]\nurl = \"https://example.test\"\n");
    let catalog = store.load_catalog().unwrap();
    let duplicate = NewServer {
        name: "docs".to_owned(),
        definition: ServerDefinition::Http {
            url: "https://other.test".to_owned(),
            bearer_token_env_var: None,
        },
        options: McpServerOptions::default(),
    };
    assert!(matches!(
        store.add_server_and_assign(&duplicate, catalog.base_fingerprint, &[]),
        Err(StoreError::ServerExists(_))
    ));

    let invalid = NewServer {
        name: "bad name".to_owned(),
        definition: ServerDefinition::Stdio {
            command: String::new(),
            args: Vec::new(),
            cwd: None,
            env_vars: vec!["NOT-VALID".to_owned()],
        },
        options: McpServerOptions::default(),
    };
    assert!(validate_new_server(&invalid).is_err());
}

#[test]
fn source_inspection_returns_native_profile_and_isolated_server_toml() {
    let (temp, store) = store_with_base(concat!(
        "# base comment\n",
        "[mcp_servers.docs]\n",
        "url = \"https://docs.example.test\"\n\n",
        "[mcp_servers.private]\n",
        "command = \"private-server\"\n",
        "args = [\"DO_NOT_INCLUDE_ELSEWHERE\"]\n",
    ));
    let profile_source = "# intentionally malformed but inspectable\nvalue = \"unfinished\n";
    fs::write(temp.path().join("broken.config.toml"), profile_source).unwrap();

    assert!(
        store
            .inspect_base_source()
            .unwrap()
            .contains("# base comment")
    );
    assert_eq!(
        store.inspect_profile_source("broken").unwrap(),
        profile_source
    );

    let server = store.inspect_server_source("docs").unwrap();
    assert!(server.contains("[mcp_servers.docs]"));
    assert!(server.contains("https://docs.example.test"));
    assert!(!server.contains("private-server"));
    assert!(!server.contains("DO_NOT_INCLUDE_ELSEWHERE"));
}

#[test]
fn updates_stdio_server_without_losing_advanced_fields_or_enablement() {
    let (temp, store) = store_with_base(concat!(
        "# preserve base comment\n",
        "[mcp_servers.local]\n",
        "# preserve server comment\n",
        "command = \"old-command\"\n",
        "args = [\"--old\"]\n",
        "cwd = \"/old\"\n",
        "env_vars = [\"OLD_TOKEN\"]\n",
        "enabled = true\n",
        "startup_timeout_sec = 45\n\n",
        "[mcp_servers.local.env]\n",
        "LOG_LEVEL = \"debug\"\n",
    ));
    let catalog = store.load_catalog().unwrap();
    let mut loaded = store.load_server_definition("local").unwrap();
    assert!(matches!(
        loaded.server.definition,
        ServerDefinition::Stdio { .. }
    ));
    loaded.server.definition = ServerDefinition::Stdio {
        command: "new-command".to_owned(),
        args: Vec::new(),
        cwd: None,
        env_vars: vec!["NEW_TOKEN".to_owned()],
    };
    let updated = ServerUpdate {
        editable: loaded,
        dirty_fields: BTreeSet::from([
            ServerField::Command,
            ServerField::Arguments,
            ServerField::WorkingDirectory,
            ServerField::EnvironmentVariables,
        ]),
    };

    store
        .update_server(&updated, catalog.base_fingerprint)
        .unwrap();

    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("# preserve base comment"));
    assert!(rendered.contains("# preserve server comment"));
    assert!(rendered.contains("command = \"new-command\""));
    assert!(rendered.contains("env_vars = [\"NEW_TOKEN\"]"));
    assert!(!rendered.contains("args ="));
    assert!(!rendered.contains("cwd ="));
    assert!(rendered.contains("enabled = true"));
    assert!(rendered.contains("startup_timeout_sec = 45"));
    assert!(rendered.contains("[mcp_servers.local.env]"));
    assert!(rendered.contains("LOG_LEVEL = \"debug\""));
}

#[test]
fn updates_http_server_without_losing_headers() {
    let (temp, store) = store_with_base(concat!(
        "[mcp_servers.remote]\n",
        "url = \"https://old.example.test/mcp\"\n",
        "bearer_token_env_var = \"OLD_TOKEN\"\n",
        "enabled = false\n\n",
        "[mcp_servers.remote.http_headers]\n",
        "X-Client = \"codex-mcp\"\n",
    ));
    let catalog = store.load_catalog().unwrap();
    let mut loaded = store.load_server_definition("remote").unwrap();
    loaded.server.definition = ServerDefinition::Http {
        url: "https://new.example.test/mcp".to_owned(),
        bearer_token_env_var: None,
    };
    let updated = ServerUpdate {
        editable: loaded,
        dirty_fields: BTreeSet::from([ServerField::Url, ServerField::BearerEnvironmentVariable]),
    };

    store
        .update_server(&updated, catalog.base_fingerprint)
        .unwrap();

    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("url = \"https://new.example.test/mcp\""));
    assert!(!rendered.contains("bearer_token_env_var"));
    assert!(rendered.contains("enabled = false"));
    assert!(rendered.contains("[mcp_servers.remote.http_headers]"));
    assert!(rendered.contains("X-Client = \"codex-mcp\""));
}

#[test]
fn server_update_refuses_stale_base_fingerprint() {
    let (temp, store) = store_with_base("[mcp_servers.docs]\nurl = \"https://old.example.test\"\n");
    let catalog = store.load_catalog().unwrap();
    fs::write(
        temp.path().join("config.toml"),
        "[mcp_servers.docs]\nurl = \"https://external.example.test\"\n",
    )
    .unwrap();
    let mut loaded = store.load_server_definition("docs").unwrap();
    loaded.server.definition = ServerDefinition::Http {
        url: "https://new.example.test".to_owned(),
        bearer_token_env_var: None,
    };
    let updated = ServerUpdate {
        editable: loaded,
        dirty_fields: BTreeSet::from([ServerField::Url]),
    };

    assert!(matches!(
        store.update_server(&updated, catalog.base_fingerprint),
        Err(StoreError::BaseConcurrentChange)
    ));
    assert!(
        fs::read_to_string(temp.path().join("config.toml"))
            .unwrap()
            .contains("external.example.test")
    );
}

#[test]
fn guided_editor_locks_structured_env_vars_without_echoing_values() {
    let (_temp, store) = store_with_base(concat!(
        "[mcp_servers.local]\n",
        "command = \"server\"\n",
        "env_vars = [{ name = \"DO_NOT_ECHO\" }]\n",
    ));

    let editable = store.load_server_definition("local").unwrap();
    assert!(
        editable
            .locked_fields
            .contains(&ServerField::EnvironmentVariables)
    );
    assert!(matches!(
        editable.server.definition,
        ServerDefinition::Stdio { ref env_vars, .. } if env_vars.is_empty()
    ));
}

#[test]
fn adds_all_supported_core_server_options() {
    let (temp, store) = store_with_base("");
    let catalog = store.load_catalog().unwrap();
    let server = NewServer {
        name: "complete".to_owned(),
        definition: ServerDefinition::Http {
            url: "https://example.test/mcp".to_owned(),
            bearer_token_env_var: None,
        },
        options: McpServerOptions {
            required: Some(true),
            startup_timeout_sec: Some(12.5),
            tool_timeout_sec: Some(90.0),
            enabled_tools: Some(Vec::new()),
            disabled_tools: Some(vec!["dangerous".to_owned()]),
            default_tools_approval_mode: Some(ToolApprovalMode::Writes),
        },
    };

    store
        .add_server_and_assign(&server, catalog.base_fingerprint, &[])
        .unwrap();

    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("required = true"));
    assert!(rendered.contains("startup_timeout_sec = 12.5"));
    assert!(rendered.contains("tool_timeout_sec = 90.0"));
    assert!(rendered.contains("enabled_tools = []"));
    assert!(rendered.contains("disabled_tools = [\"dangerous\"]"));
    assert!(rendered.contains("default_tools_approval_mode = \"writes\""));
}

#[test]
fn preserves_legacy_startup_milliseconds_until_timeout_is_edited() {
    let (temp, store) = store_with_base(concat!(
        "[mcp_servers.local]\n",
        "command = \"server\"\n",
        "startup_timeout_ms = 1500\n",
    ));
    let catalog = store.load_catalog().unwrap();
    let mut editable = store.load_server_definition("local").unwrap();
    assert_eq!(
        editable.startup_timeout_source,
        StartupTimeoutSource::Milliseconds
    );
    assert_eq!(editable.server.options.startup_timeout_sec, Some(1.5));

    editable.server.options.required = Some(true);
    store
        .update_server(
            &ServerUpdate {
                editable,
                dirty_fields: BTreeSet::from([ServerField::Required]),
            },
            catalog.base_fingerprint,
        )
        .unwrap();
    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("startup_timeout_ms = 1500"));
    assert!(!rendered.contains("startup_timeout_sec"));

    let catalog = store.load_catalog().unwrap();
    let mut editable = store.load_server_definition("local").unwrap();
    editable.server.options.startup_timeout_sec = Some(2.25);
    store
        .update_server(
            &ServerUpdate {
                editable,
                dirty_fields: BTreeSet::from([ServerField::StartupTimeout]),
            },
            catalog.base_fingerprint,
        )
        .unwrap();
    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(!rendered.contains("startup_timeout_ms"));
    assert!(rendered.contains("startup_timeout_sec = 2.25"));
}

#[test]
fn tool_lists_distinguish_unset_from_explicit_empty() {
    let (temp, store) = store_with_base("[mcp_servers.local]\ncommand = \"server\"\n");
    let catalog = store.load_catalog().unwrap();
    let mut editable = store.load_server_definition("local").unwrap();
    assert_eq!(editable.server.options.enabled_tools, None);
    editable.server.options.enabled_tools = Some(Vec::new());
    store
        .update_server(
            &ServerUpdate {
                editable,
                dirty_fields: BTreeSet::from([ServerField::EnabledTools]),
            },
            catalog.base_fingerprint,
        )
        .unwrap();
    assert!(
        fs::read_to_string(temp.path().join("config.toml"))
            .unwrap()
            .contains("enabled_tools = []")
    );
    let loaded = store.load_server_definition("local").unwrap();
    assert_eq!(loaded.server.options.enabled_tools, Some(Vec::new()));

    let catalog = store.load_catalog().unwrap();
    let mut editable = loaded;
    editable.server.options.enabled_tools = None;
    store
        .update_server(
            &ServerUpdate {
                editable,
                dirty_fields: BTreeSet::from([ServerField::EnabledTools]),
            },
            catalog.base_fingerprint,
        )
        .unwrap();
    assert!(
        !fs::read_to_string(temp.path().join("config.toml"))
            .unwrap()
            .contains("enabled_tools")
    );
}

#[test]
fn editing_core_option_preserves_structured_env_vars() {
    let (temp, store) = store_with_base(concat!(
        "[mcp_servers.local]\n",
        "command = \"server\"\n",
        "env_vars = [{ name = \"TOKEN\", value = \"SECRET\" }]\n",
    ));
    let catalog = store.load_catalog().unwrap();
    let mut editable = store.load_server_definition("local").unwrap();
    editable.server.options.tool_timeout_sec = Some(60.0);
    store
        .update_server(
            &ServerUpdate {
                editable,
                dirty_fields: BTreeSet::from([ServerField::ToolTimeout]),
            },
            catalog.base_fingerprint,
        )
        .unwrap();

    let rendered = fs::read_to_string(temp.path().join("config.toml")).unwrap();
    assert!(rendered.contains("env_vars = [{ name = \"TOKEN\", value = \"SECRET\" }]"));
    assert!(rendered.contains("tool_timeout_sec = 60.0"));
}
