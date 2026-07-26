use super::*;

pub(super) fn extract_server_definition(
    document: &DocumentMut,
    name: &str,
    path: &Path,
) -> Result<NewServer, StoreError> {
    let Some(mcp_servers) = document.get("mcp_servers").and_then(Item::as_table_like) else {
        return Err(StoreError::ServerMissing(name.to_owned()));
    };
    let Some(server) = mcp_servers.get(name).and_then(Item::as_table_like) else {
        return Err(StoreError::ServerMissing(name.to_owned()));
    };

    let definition = match (server.contains_key("command"), server.contains_key("url")) {
        (true, false) => ServerDefinition::Stdio {
            command: required_string(server.get("command"), name, "command", path)?,
            args: optional_string_array(server.get("args"), name, "args", path)?,
            cwd: optional_string(server.get("cwd"), name, "cwd", path)?,
            env_vars: optional_string_array(server.get("env_vars"), name, "env_vars", path)?,
        },
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

    let server = NewServer {
        name: name.to_owned(),
        definition,
    };
    validate_new_server(&server)?;
    Ok(server)
}

pub(super) fn update_server_definition(
    document: &mut DocumentMut,
    updated: &NewServer,
    path: &Path,
) -> Result<(), StoreError> {
    let current = extract_server_definition(document, &updated.name, path)?;
    if std::mem::discriminant(&current.definition) != std::mem::discriminant(&updated.definition) {
        return Err(StoreError::InvalidServer(
            "changing a server's transport is not supported by the guided editor".to_owned(),
        ));
    }

    let mcp_servers = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: "mcp_servers must be a standard TOML table to use the guided editor"
                .to_owned(),
        })?;
    let table = mcp_servers
        .get_mut(&updated.name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: format!(
                "MCP server '{}' must be a standard TOML table to use the guided editor",
                updated.name
            ),
        })?;

    match &updated.definition {
        ServerDefinition::Stdio {
            command,
            args,
            cwd,
            env_vars,
        } => {
            table["command"] = value(command.clone());
            set_optional_array(table, "args", args);
            set_optional_string(table, "cwd", cwd.as_deref());
            set_optional_array(table, "env_vars", env_vars);
        }
        ServerDefinition::Http {
            url,
            bearer_token_env_var,
        } => {
            table["url"] = value(url.clone());
            set_optional_string(
                table,
                "bearer_token_env_var",
                bearer_token_env_var.as_deref(),
            );
        }
    }
    Ok(())
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

fn optional_string_array(
    item: Option<&Item>,
    server: &str,
    field: &str,
    path: &Path,
) -> Result<Vec<String>, StoreError> {
    let Some(item) = item else {
        return Ok(Vec::new());
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
        .collect()
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

fn set_optional_array(table: &mut Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
    } else {
        table[key] = Item::Value(Value::Array(string_array(values)));
    }
}
