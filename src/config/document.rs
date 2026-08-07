use super::*;

pub(super) struct LoadedDocument {
    pub(super) document: DocumentMut,
    pub(super) raw: Vec<u8>,
    pub(super) fingerprint: Fingerprint,
}

pub(super) fn read_optional_document(path: &Path) -> Result<LoadedDocument, StoreError> {
    match fs::read(path) {
        Ok(raw) => parse_document(path, raw),
        Err(error) if error.kind() == io::ErrorKind::NotFound => parse_document(path, Vec::new()),
        Err(source) => Err(StoreError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

pub(super) fn read_optional_source(path: &Path) -> Result<String, StoreError> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(StoreError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

pub(super) fn read_required_document(path: &Path) -> Result<LoadedDocument, StoreError> {
    let raw = fs::read(path).map_err(|source| StoreError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse_document(path, raw)
}

pub(super) fn parse_document(path: &Path, raw: Vec<u8>) -> Result<LoadedDocument, StoreError> {
    let source = std::str::from_utf8(&raw).map_err(|_| StoreError::Parse {
        path: path.to_owned(),
    })?;
    let document = source
        .parse::<DocumentMut>()
        .map_err(|_| StoreError::Parse {
            path: path.to_owned(),
        })?;
    let fingerprint = fingerprint(&raw);
    Ok(LoadedDocument {
        document,
        raw,
        fingerprint,
    })
}

pub(super) fn fingerprint(raw: &[u8]) -> Fingerprint {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    Fingerprint(hasher.finish())
}

pub(super) fn extract_server_facts(
    document: &DocumentMut,
    path: &Path,
) -> Result<BTreeMap<String, ServerFacts>, StoreError> {
    let Some(item) = document.get("mcp_servers") else {
        return Ok(BTreeMap::new());
    };
    let Some(table) = item.as_table_like() else {
        return Err(StoreError::InvalidConfig {
            path: path.to_owned(),
            message: "mcp_servers must be a table".to_owned(),
        });
    };

    let mut servers = BTreeMap::new();
    for (name, item) in table.iter() {
        let Some(server) = item.as_table_like() else {
            return Err(StoreError::InvalidConfig {
                path: path.to_owned(),
                message: format!("MCP server '{name}' must be a table"),
            });
        };
        let enabled_item = server.get("enabled");
        let enabled = enabled_item.and_then(Item::as_bool);
        let enabled_is_valid = enabled_item.is_none() || enabled.is_some();
        servers.insert(
            name.to_owned(),
            ServerFacts {
                enabled,
                enabled_is_valid,
                transport: match (server.contains_key("command"), server.contains_key("url")) {
                    (true, false) => TransportKind::Stdio,
                    (false, true) => TransportKind::Http,
                    _ => TransportKind::Invalid,
                },
            },
        );
    }
    Ok(servers)
}

pub(super) fn base_views(base: &BTreeMap<String, ServerFacts>) -> Vec<ServerView> {
    base.iter()
        .map(|(name, facts)| ServerView {
            name: name.clone(),
            state: OverrideState::Inherited,
            effective_enabled: facts.enabled.unwrap_or(true),
            inherited_enabled: facts.enabled.unwrap_or(true),
            origin: if facts.transport != TransportKind::Invalid {
                ServerOrigin::Base
            } else {
                ServerOrigin::OverrideOnly
            },
            transport: facts.transport,
            valid: facts.enabled_is_valid && facts.transport != TransportKind::Invalid,
        })
        .collect()
}

pub(super) fn merge_server_views(
    base: &BTreeMap<String, ServerFacts>,
    profile: &BTreeMap<String, ServerFacts>,
) -> Vec<ServerView> {
    let names = base
        .keys()
        .chain(profile.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    names
        .into_iter()
        .map(|name| {
            let base_facts = base.get(&name);
            let profile_facts = profile.get(&name);
            let inherited_enabled = base_facts.and_then(|facts| facts.enabled).unwrap_or(true);
            let explicit = profile_facts.and_then(|facts| facts.enabled);
            let state = match explicit {
                Some(true) => OverrideState::Enabled,
                Some(false) => OverrideState::Disabled,
                None => OverrideState::Inherited,
            };
            let effective_enabled = explicit.unwrap_or(inherited_enabled);
            let base_definition =
                base_facts.is_some_and(|facts| facts.transport != TransportKind::Invalid);
            let profile_definition =
                profile_facts.is_some_and(|facts| facts.transport != TransportKind::Invalid);
            let enabled_valid = base_facts.is_none_or(|facts| facts.enabled_is_valid)
                && profile_facts.is_none_or(|facts| facts.enabled_is_valid);
            let origin = if base_definition {
                ServerOrigin::Base
            } else if profile_definition {
                ServerOrigin::Profile
            } else {
                ServerOrigin::OverrideOnly
            };
            let transport = base_facts
                .map(|facts| facts.transport)
                .filter(|transport| *transport != TransportKind::Invalid)
                .or_else(|| {
                    profile_facts
                        .map(|facts| facts.transport)
                        .filter(|transport| *transport != TransportKind::Invalid)
                })
                .unwrap_or(TransportKind::Invalid);

            ServerView {
                name,
                state,
                effective_enabled,
                inherited_enabled,
                origin,
                transport,
                valid: enabled_valid && (base_definition || profile_definition),
            }
        })
        .collect()
}

pub(super) fn set_server_override(
    document: &mut DocumentMut,
    server_name: &str,
    state: OverrideState,
    path: &Path,
) -> Result<(), StoreError> {
    if state == OverrideState::Inherited {
        let Some(mcp_servers) = document.get_mut("mcp_servers").and_then(Item::as_table_mut) else {
            return Ok(());
        };
        let should_remove = if let Some(server) = mcp_servers
            .get_mut(server_name)
            .and_then(Item::as_table_mut)
        {
            server.remove("enabled");
            server.is_empty()
        } else {
            false
        };
        if should_remove {
            mcp_servers.remove(server_name);
        }
        return Ok(());
    }

    if document.get("mcp_servers").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        document["mcp_servers"] = Item::Table(table);
    }
    let mcp_servers = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: "mcp_servers must be a table".to_owned(),
        })?;
    if mcp_servers.get(server_name).is_none() {
        mcp_servers.insert(server_name, Item::Table(Table::new()));
    }
    let server = mcp_servers
        .get_mut(server_name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: format!("MCP server '{server_name}' must be a table"),
        })?;
    server["enabled"] = value(state == OverrideState::Enabled);
    Ok(())
}

pub(super) fn insert_server_definition(
    document: &mut DocumentMut,
    server: &NewServer,
    path: &Path,
) -> Result<(), StoreError> {
    if document.get("mcp_servers").is_none() {
        let mut table = Table::new();
        table.set_implicit(true);
        document["mcp_servers"] = Item::Table(table);
    }
    let mcp_servers = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| StoreError::InvalidConfig {
            path: path.to_owned(),
            message: "mcp_servers must be a table".to_owned(),
        })?;
    if mcp_servers.contains_key(&server.name) {
        return Err(StoreError::ServerExists(server.name.clone()));
    }

    let mut table = Table::new();
    match &server.definition {
        ServerDefinition::Stdio {
            command,
            args,
            cwd,
            env_vars,
        } => {
            table["command"] = value(command.clone());
            if !args.is_empty() {
                table["args"] = Item::Value(Value::Array(string_array(args)));
            }
            if let Some(cwd) = cwd {
                table["cwd"] = value(cwd.clone());
            }
            if !env_vars.is_empty() {
                table["env_vars"] = Item::Value(Value::Array(string_array(env_vars)));
            }
        }
        ServerDefinition::Http {
            url,
            bearer_token_env_var,
        } => {
            table["url"] = value(url.clone());
            if let Some(variable) = bearer_token_env_var {
                table["bearer_token_env_var"] = value(variable.clone());
            }
        }
    }
    insert_server_options(&mut table, &server.options);
    table["enabled"] = value(false);
    mcp_servers.insert(&server.name, Item::Table(table));
    Ok(())
}

fn insert_server_options(table: &mut Table, options: &McpServerOptions) {
    if let Some(required) = options.required {
        table["required"] = value(required);
    }
    if let Some(timeout) = options.startup_timeout_sec {
        table["startup_timeout_sec"] = value(timeout);
    }
    if let Some(timeout) = options.tool_timeout_sec {
        table["tool_timeout_sec"] = value(timeout);
    }
    if let Some(tools) = &options.enabled_tools {
        table["enabled_tools"] = Item::Value(Value::Array(string_array(tools)));
    }
    if let Some(tools) = &options.disabled_tools {
        table["disabled_tools"] = Item::Value(Value::Array(string_array(tools)));
    }
    if let Some(mode) = options.default_tools_approval_mode {
        table["default_tools_approval_mode"] = value(mode.as_str());
    }
}

pub(super) fn string_array(values: &[String]) -> Array {
    let mut array = Array::new();
    for value in values {
        array.push(value.as_str());
    }
    array
}

pub fn validate_new_server(server: &NewServer) -> Result<(), StoreError> {
    validate_server_name(&server.name)?;
    match &server.definition {
        ServerDefinition::Stdio {
            command,
            cwd,
            env_vars,
            ..
        } => {
            if command.trim().is_empty() {
                return Err(StoreError::InvalidServer(
                    "stdio command cannot be empty".to_owned(),
                ));
            }
            if cwd.as_ref().is_some_and(|cwd| cwd.trim().is_empty()) {
                return Err(StoreError::InvalidServer(
                    "working directory cannot be blank".to_owned(),
                ));
            }
            for variable in env_vars {
                validate_env_var(variable)?;
            }
        }
        ServerDefinition::Http {
            url,
            bearer_token_env_var,
        } => {
            let trimmed = url.trim();
            let endpoint = trimmed.split_once("://").map(|(_, endpoint)| endpoint);
            if !(trimmed.starts_with("https://") || trimmed.starts_with("http://"))
                || trimmed.contains(char::is_whitespace)
                || endpoint.is_none_or(|endpoint| endpoint.is_empty() || endpoint.starts_with('/'))
            {
                return Err(StoreError::InvalidServer(
                    "HTTP URL must contain a host, start with http:// or https://, and contain no spaces"
                        .to_owned(),
                ));
            }
            if let Some(variable) = bearer_token_env_var {
                validate_env_var(variable)?;
            }
        }
    }
    validate_server_options(&server.options)?;
    Ok(())
}

pub(super) fn validate_server_options(options: &McpServerOptions) -> Result<(), StoreError> {
    for (label, timeout) in [
        ("startup timeout", options.startup_timeout_sec),
        ("tool timeout", options.tool_timeout_sec),
    ] {
        if timeout.is_some_and(|timeout| !timeout.is_finite() || timeout < 0.0) {
            return Err(StoreError::InvalidServer(format!(
                "{label} must be a finite, non-negative number"
            )));
        }
    }
    if let Some(tools) = &options.enabled_tools {
        validate_tool_names("enabled tools", tools)?;
    }
    if let Some(tools) = &options.disabled_tools {
        validate_tool_names("disabled tools", tools)?;
    }
    Ok(())
}

fn validate_tool_names(label: &str, tools: &[String]) -> Result<(), StoreError> {
    let mut seen = BTreeSet::new();
    for tool in tools {
        if tool.trim().is_empty() {
            return Err(StoreError::InvalidServer(format!(
                "{label} cannot contain a blank tool name"
            )));
        }
        if !seen.insert(tool) {
            return Err(StoreError::InvalidServer(format!(
                "{label} cannot contain duplicate tool names"
            )));
        }
    }
    Ok(())
}

pub fn validate_server_name(name: &str) -> Result<(), StoreError> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(())
    } else {
        Err(StoreError::InvalidServerName)
    }
}

pub(super) fn validate_env_var(name: &str) -> Result<(), StoreError> {
    let mut bytes = name.bytes();
    let valid_start = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if valid_start && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        Ok(())
    } else {
        Err(StoreError::InvalidServer(format!(
            "'{name}' is not a valid environment variable name"
        )))
    }
}

pub(super) fn atomic_write(
    path: &Path,
    contents: &str,
    permissions: Option<fs::Permissions>,
) -> Result<(), StoreError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| StoreError::Write {
        path: path.to_owned(),
        source,
    })?;
    temporary
        .write_all(contents.as_bytes())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| StoreError::Write {
            path: path.to_owned(),
            source,
        })?;

    if let Some(permissions) = permissions {
        fs::set_permissions(temporary.path(), permissions).map_err(|source| StoreError::Write {
            path: path.to_owned(),
            source,
        })?;
    } else {
        set_private_file_permissions(temporary.as_file(), path)?;
    }

    temporary.persist(path).map_err(|error| StoreError::Write {
        path: path.to_owned(),
        source: error.error,
    })?;
    sync_parent(parent);
    Ok(())
}

pub(super) fn profile_name_from_path(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let name = filename.strip_suffix(PROFILE_SUFFIX)?;
    if validate_profile_name(name).is_ok() {
        Some(name.to_owned())
    } else {
        None
    }
}

pub(super) fn validate_profile_name(name: &str) -> Result<(), StoreError> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Ok(())
    } else {
        Err(StoreError::InvalidProfileName)
    }
}

#[cfg(unix)]
pub(super) fn set_private_file_permissions(file: &File, path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| StoreError::Write {
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
pub(super) fn set_private_file_permissions(_file: &File, _path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_private_dir_permissions(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        StoreError::Write {
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(not(unix))]
pub(super) fn set_private_dir_permissions(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

pub(super) fn sync_parent(path: &Path) {
    if let Ok(directory) = OpenOptions::new().read(true).open(path) {
        let _ = directory.sync_all();
    }
}
