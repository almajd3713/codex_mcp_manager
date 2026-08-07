use super::*;

pub(super) fn extract_server_definition(
    document: &DocumentMut,
    name: &str,
    path: &Path,
) -> Result<EditableServer, StoreError> {
    let Some(mcp_servers) = document.get("mcp_servers").and_then(Item::as_table_like) else {
        return Err(StoreError::ServerMissing(name.to_owned()));
    };
    let Some(server) = mcp_servers.get(name).and_then(Item::as_table_like) else {
        return Err(StoreError::ServerMissing(name.to_owned()));
    };

    let mut locked_fields = BTreeSet::new();
    let definition = match (server.contains_key("command"), server.contains_key("url")) {
        (true, false) => {
            let (env_vars, env_vars_locked) =
                editable_string_array(server.get("env_vars"), name, "env_vars", path)?;
            if env_vars_locked {
                locked_fields.insert(ServerField::EnvironmentVariables);
            }
            ServerDefinition::Stdio {
                command: required_string(server.get("command"), name, "command", path)?,
                args: optional_string_array(server.get("args"), name, "args", path)?
                    .unwrap_or_default(),
                cwd: optional_string(server.get("cwd"), name, "cwd", path)?,
                env_vars,
            }
        }
        (false, true) => ServerDefinition::Http {
            url: required_string(server.get("url"), name, "url", path)?,
            bearer_token_env_var: optional_string(
                server.get("bearer_token_env_var"),
                name,
                "bearer_token_env_var",
                path,
            )?,
        },
        _ => {
            return Err(StoreError::InvalidConfig {
                path: path.to_owned(),
                message: format!(
                    "MCP server '{name}' must define exactly one of command or url to use the guided editor"
                ),
            });
        }
    };

    let startup_seconds = optional_number(
        server.get("startup_timeout_sec"),
        name,
        "startup_timeout_sec",
        path,
    )?;
    let startup_milliseconds = optional_nonnegative_integer(
        server.get("startup_timeout_ms"),
        name,
        "startup_timeout_ms",
        path,
    )?;
    let (startup_timeout_sec, startup_timeout_source) = match (
        startup_seconds,
        startup_milliseconds,
    ) {
        (Some(_), Some(_)) => {
            return Err(StoreError::InvalidConfig {
                path: path.to_owned(),
                message: format!(
                    "MCP server '{name}' cannot define both startup_timeout_sec and startup_timeout_ms"
                ),
            });
        }
        (Some(seconds), None) => (Some(seconds), StartupTimeoutSource::Seconds),
        (None, Some(milliseconds)) => (
            Some(milliseconds as f64 / 1_000.0),
            StartupTimeoutSource::Milliseconds,
        ),
        (None, None) => (None, StartupTimeoutSource::None),
    };

    let options = McpServerOptions {
        required: optional_bool(server.get("required"), name, "required", path)?,
        startup_timeout_sec,
        tool_timeout_sec: optional_number(
            server.get("tool_timeout_sec"),
            name,
            "tool_timeout_sec",
            path,
        )?,
        enabled_tools: optional_string_array(
            server.get("enabled_tools"),
            name,
            "enabled_tools",
            path,
        )?,
        disabled_tools: optional_string_array(
            server.get("disabled_tools"),
            name,
            "disabled_tools",
            path,
        )?,
        default_tools_approval_mode: optional_approval_mode(
            server.get("default_tools_approval_mode"),
            name,
            path,
        )?,
    };
    validate_server_options(&options)?;

    Ok(EditableServer {
        server: NewServer {
            name: name.to_owned(),
            definition,
            options,
        },
        locked_fields,
        startup_timeout_source,
    })
}

pub(super) fn update_server_definition(
    document: &mut DocumentMut,
    update: &ServerUpdate,
    path: &Path,
) -> Result<(), StoreError> {
    validate_update(update)?;
    let current = extract_server_definition(document, &update.editable.server.name, path)?;
    if std::mem::discriminant(&current.server.definition)
        != std::mem::discriminant(&update.editable.server.definition)
    {
        return Err(StoreError::InvalidServer(
            "changing a server's transport is not supported by the guided editor".to_owned(),
        ));
    }

    let server_name = &update.editable.server.name;
    let mcp_servers = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: "mcp_servers must be a standard TOML table to use the guided editor"
                .to_owned(),
        })?;
    let table = mcp_servers
        .get_mut(server_name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: format!(
                "MCP server '{server_name}' must be a standard TOML table to use the guided editor"
            ),
        })?;

    let fields = &update.dirty_fields;
    match &update.editable.server.definition {
        ServerDefinition::Stdio {
            command,
            args,
            cwd,
            env_vars,
        } => {
            if fields.contains(&ServerField::Command) {
                table["command"] = value(command.clone());
            }
            if fields.contains(&ServerField::Arguments) {
                set_optional_array(table, "args", (!args.is_empty()).then_some(args));
            }
            if fields.contains(&ServerField::WorkingDirectory) {
                set_optional_string(table, "cwd", cwd.as_deref());
            }
            if fields.contains(&ServerField::EnvironmentVariables) {
                set_optional_array(
                    table,
                    "env_vars",
                    (!env_vars.is_empty()).then_some(env_vars),
                );
            }
        }
        ServerDefinition::Http {
            url,
            bearer_token_env_var,
        } => {
            if fields.contains(&ServerField::Url) {
                table["url"] = value(url.clone());
            }
            if fields.contains(&ServerField::BearerEnvironmentVariable) {
                set_optional_string(
                    table,
                    "bearer_token_env_var",
                    bearer_token_env_var.as_deref(),
                );
            }
        }
    }

    let options = &update.editable.server.options;
    if fields.contains(&ServerField::Required) {
        set_optional_bool(table, "required", options.required);
    }
    if fields.contains(&ServerField::StartupTimeout) {
        table.remove("startup_timeout_ms");
        set_optional_number(table, "startup_timeout_sec", options.startup_timeout_sec);
    }
    if fields.contains(&ServerField::ToolTimeout) {
        set_optional_number(table, "tool_timeout_sec", options.tool_timeout_sec);
    }
    if fields.contains(&ServerField::EnabledTools) {
        set_optional_array(table, "enabled_tools", options.enabled_tools.as_ref());
    }
    if fields.contains(&ServerField::DisabledTools) {
        set_optional_array(table, "disabled_tools", options.disabled_tools.as_ref());
    }
    if fields.contains(&ServerField::DefaultToolsApprovalMode) {
        set_optional_string(
            table,
            "default_tools_approval_mode",
            options
                .default_tools_approval_mode
                .map(ToolApprovalMode::as_str),
        );
    }
    Ok(())
}

fn validate_update(update: &ServerUpdate) -> Result<(), StoreError> {
    let fields = &update.dirty_fields;
    let server = &update.editable.server;
    if fields.iter().any(|field| {
        matches!(
            field,
            ServerField::Command
                | ServerField::Arguments
                | ServerField::WorkingDirectory
                | ServerField::EnvironmentVariables
                | ServerField::Url
                | ServerField::BearerEnvironmentVariable
        )
    }) {
        validate_new_server(&NewServer {
            name: server.name.clone(),
            definition: server.definition.clone(),
            options: McpServerOptions::default(),
        })?;
    }
    let mut changed_options = McpServerOptions::default();
    if fields.contains(&ServerField::StartupTimeout) {
        changed_options.startup_timeout_sec = server.options.startup_timeout_sec;
    }
    if fields.contains(&ServerField::ToolTimeout) {
        changed_options.tool_timeout_sec = server.options.tool_timeout_sec;
    }
    if fields.contains(&ServerField::EnabledTools) {
        changed_options.enabled_tools = server.options.enabled_tools.clone();
    }
    if fields.contains(&ServerField::DisabledTools) {
        changed_options.disabled_tools = server.options.disabled_tools.clone();
    }
    validate_server_options(&changed_options)
}

fn required_string(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<String, StoreError> {
    item.and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_server_field(server, field, "must be a string", path))
}

fn optional_string(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Option<String>, StoreError> {
    item.map(|item| {
        item.as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid_server_field(server, field, "must be a string", path))
    })
    .transpose()
}

fn optional_bool(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Option<bool>, StoreError> {
    item.map(|item| {
        item.as_bool()
            .ok_or_else(|| invalid_server_field(server, field, "must be a boolean", path))
    })
    .transpose()
}

fn optional_number(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Option<f64>, StoreError> {
    item.map(|item| {
        item.as_float()
            .or_else(|| item.as_integer().map(|value| value as f64))
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                invalid_server_field(server, field, "must be a non-negative number", path)
            })
    })
    .transpose()
}

fn optional_nonnegative_integer(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Option<u64>, StoreError> {
    item.map(|item| {
        item.as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                invalid_server_field(server, field, "must be a non-negative integer", path)
            })
    })
    .transpose()
}

fn optional_string_array(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Option<Vec<String>>, StoreError> {
    let Some(item) = item else {
        return Ok(None);
    };
    let array = item
        .as_array()
        .ok_or_else(|| invalid_server_field(server, field, "must be an array of strings", path))?;
    array
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                invalid_server_field(server, field, "must contain only strings", path)
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn editable_string_array(
    item: Option<&Item>,
    _server: &str,
    _field: &str,
    _path: &Path,
) -> Result<(Vec<String>, bool), StoreError> {
    let Some(item) = item else {
        return Ok((Vec::new(), false));
    };
    let Some(array) = item.as_array() else {
        return Ok((Vec::new(), true));
    };
    let values = array
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>();
    Ok(values.map_or_else(|| (Vec::new(), true), |values| (values, false)))
}

fn optional_approval_mode(
    item: Option<&Item>,
    server: &str,
    path: &Path,
) -> Result<Option<ToolApprovalMode>, StoreError> {
    let Some(item) = item else {
        return Ok(None);
    };
    let mode = match item.as_str() {
        Some("auto") => ToolApprovalMode::Auto,
        Some("prompt") => ToolApprovalMode::Prompt,
        Some("writes") => ToolApprovalMode::Writes,
        Some("approve") => ToolApprovalMode::Approve,
        _ => {
            return Err(invalid_server_field(
                server,
                "default_tools_approval_mode",
                "must be auto, prompt, writes, or approve",
                path,
            ));
        }
    };
    Ok(Some(mode))
}

fn invalid_server_field(server: &str, field: &str, message: &str, path: &Path) -> StoreError {
    StoreError::InvalidConfig {
        path: path.to_owned(),
        message: format!("MCP server '{server}' field '{field}' {message}"),
    }
}

fn set_optional_string(table: &mut Table, key: &str, value_to_set: Option<&str>) {
    if let Some(value_to_set) = value_to_set {
        table[key] = value(value_to_set);
    } else {
        table.remove(key);
    }
}

fn set_optional_bool(table: &mut Table, key: &str, value_to_set: Option<bool>) {
    if let Some(value_to_set) = value_to_set {
        table[key] = value(value_to_set);
    } else {
        table.remove(key);
    }
}

fn set_optional_number(table: &mut Table, key: &str, value_to_set: Option<f64>) {
    if let Some(value_to_set) = value_to_set {
        table[key] = value(value_to_set);
    } else {
        table.remove(key);
    }
}

fn set_optional_array(table: &mut Table, key: &str, values: Option<&Vec<String>>) {
    if let Some(values) = values {
        table[key] = Item::Value(Value::Array(string_array(values)));
    } else {
        table.remove(key);
    }
}
