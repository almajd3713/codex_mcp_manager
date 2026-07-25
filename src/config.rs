use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::NamedTempFile;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

const PROFILE_SUFFIX: &str = ".config.toml";
const TRASH_DIR: &str = ".codex-mcp-trash";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverrideState {
    Inherited,
    Enabled,
    Disabled,
}

impl OverrideState {
    pub fn cycle(self) -> Self {
        match self {
            Self::Inherited => Self::Enabled,
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::Inherited,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerOrigin {
    Base,
    Profile,
    OverrideOnly,
}

#[derive(Clone, Debug)]
pub struct ServerView {
    pub name: String,
    pub state: OverrideState,
    pub effective_enabled: bool,
    pub inherited_enabled: bool,
    pub origin: ServerOrigin,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fingerprint(u64);

#[derive(Clone, Debug)]
pub struct ProfileRecord {
    pub name: String,
    pub fingerprint: Fingerprint,
    pub servers: Vec<ServerView>,
    pub error: Option<String>,
}

impl ProfileRecord {
    pub fn enabled_count(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.effective_enabled && server.valid)
            .count()
    }
}

#[derive(Clone, Debug)]
pub struct Catalog {
    pub base_servers: Vec<ServerView>,
    pub profiles: Vec<ProfileRecord>,
}

#[derive(Clone, Debug)]
struct ServerFacts {
    enabled: Option<bool>,
    enabled_is_valid: bool,
    has_definition: bool,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not parse TOML in {path}; the file was not changed")]
    Parse { path: PathBuf },
    #[error("invalid MCP configuration in {path}: {message}")]
    InvalidConfig { path: PathBuf, message: String },
    #[error("profile name must contain only letters, numbers, hyphens, and underscores")]
    InvalidProfileName,
    #[error("profile '{0}' already exists")]
    ProfileExists(String),
    #[error("profile '{0}' does not exist")]
    ProfileMissing(String),
    #[error("profile '{0}' changed on disk; reload before saving")]
    ConcurrentChange(String),
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

#[derive(Clone, Debug)]
pub struct ConfigStore {
    home: PathBuf,
}

impl ConfigStore {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn load_catalog(&self) -> Result<Catalog, StoreError> {
        let base_path = self.home.join("config.toml");
        let base_doc = read_optional_document(&base_path)?;
        let base_facts = extract_server_facts(&base_doc.document, &base_path)?;
        let base_servers = base_views(&base_facts);

        let mut profiles = Vec::new();
        let entries = match fs::read_dir(&self.home) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Catalog {
                    base_servers,
                    profiles,
                });
            }
            Err(source) => {
                return Err(StoreError::Read {
                    path: self.home.clone(),
                    source,
                });
            }
        };

        for entry in entries {
            let entry = entry.map_err(|source| StoreError::Read {
                path: self.home.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(name) = profile_name_from_path(&path) else {
                continue;
            };

            match read_required_document(&path) {
                Ok(profile_doc) => match extract_server_facts(&profile_doc.document, &path) {
                    Ok(profile_facts) => profiles.push(ProfileRecord {
                        name,
                        fingerprint: profile_doc.fingerprint,
                        servers: merge_server_views(&base_facts, &profile_facts),
                        error: None,
                    }),
                    Err(error) => profiles.push(ProfileRecord {
                        name,
                        fingerprint: profile_doc.fingerprint,
                        servers: Vec::new(),
                        error: Some(error.to_string()),
                    }),
                },
                Err(error) => profiles.push(ProfileRecord {
                    name,
                    fingerprint: Fingerprint(0),
                    servers: Vec::new(),
                    error: Some(error.to_string()),
                }),
            }
        }
        profiles.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(Catalog {
            base_servers,
            profiles,
        })
    }

    pub fn create_profile(&self, name: &str) -> Result<(), StoreError> {
        validate_profile_name(name)?;
        let home_already_existed = self.home.exists();
        fs::create_dir_all(&self.home).map_err(|source| StoreError::Write {
            path: self.home.clone(),
            source,
        })?;
        if !home_already_existed {
            set_private_dir_permissions(&self.home)?;
        }

        let path = self.profile_path(name);
        if path.exists() {
            return Err(StoreError::ProfileExists(name.to_owned()));
        }
        atomic_write(&path, "", None)
    }

    pub fn save_profile_states(
        &self,
        name: &str,
        expected: Fingerprint,
        states: &BTreeMap<String, OverrideState>,
    ) -> Result<(), StoreError> {
        validate_profile_name(name)?;
        let path = self.profile_path(name);
        let loaded = read_required_document(&path)?;
        if loaded.fingerprint != expected {
            return Err(StoreError::ConcurrentChange(name.to_owned()));
        }

        let mut document = loaded.document;
        for (server_name, state) in states {
            set_server_override(&mut document, server_name, *state, &path)?;
        }

        let rendered = document.to_string();
        if rendered.as_bytes() == loaded.raw.as_slice() {
            return Ok(());
        }

        let permissions = fs::metadata(&path)
            .ok()
            .map(|metadata| metadata.permissions());
        atomic_write(&path, &rendered, permissions)
    }

    pub fn rename_profile(&self, old_name: &str, new_name: &str) -> Result<(), StoreError> {
        validate_profile_name(old_name)?;
        validate_profile_name(new_name)?;
        if old_name == new_name {
            return Ok(());
        }

        let old_path = self.profile_path(old_name);
        let new_path = self.profile_path(new_name);
        if !old_path.exists() {
            return Err(StoreError::ProfileMissing(old_name.to_owned()));
        }
        if new_path.exists() {
            return Err(StoreError::ProfileExists(new_name.to_owned()));
        }

        fs::rename(&old_path, &new_path).map_err(|source| StoreError::Write {
            path: new_path,
            source,
        })
    }

    pub fn trash_profile(&self, name: &str) -> Result<PathBuf, StoreError> {
        validate_profile_name(name)?;
        let source_path = self.profile_path(name);
        if !source_path.exists() {
            return Err(StoreError::ProfileMissing(name.to_owned()));
        }

        let trash_dir = self.home.join(TRASH_DIR);
        fs::create_dir_all(&trash_dir).map_err(|source| StoreError::Write {
            path: trash_dir.clone(),
            source,
        })?;
        set_private_dir_permissions(&trash_dir)?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut destination = trash_dir.join(format!("{timestamp}-{name}{PROFILE_SUFFIX}"));
        let mut collision = 1_u32;
        while destination.exists() {
            destination = trash_dir.join(format!("{timestamp}-{collision}-{name}{PROFILE_SUFFIX}"));
            collision += 1;
        }

        fs::rename(&source_path, &destination).map_err(|source| StoreError::Write {
            path: destination.clone(),
            source,
        })?;
        Ok(destination)
    }

    fn profile_path(&self, name: &str) -> PathBuf {
        self.home.join(format!("{name}{PROFILE_SUFFIX}"))
    }
}

struct LoadedDocument {
    document: DocumentMut,
    raw: Vec<u8>,
    fingerprint: Fingerprint,
}

fn read_optional_document(path: &Path) -> Result<LoadedDocument, StoreError> {
    match fs::read(path) {
        Ok(raw) => parse_document(path, raw),
        Err(error) if error.kind() == io::ErrorKind::NotFound => parse_document(path, Vec::new()),
        Err(source) => Err(StoreError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn read_required_document(path: &Path) -> Result<LoadedDocument, StoreError> {
    let raw = fs::read(path).map_err(|source| StoreError::Read {
        path: path.to_owned(),
        source,
    })?;
    parse_document(path, raw)
}

fn parse_document(path: &Path, raw: Vec<u8>) -> Result<LoadedDocument, StoreError> {
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

fn fingerprint(raw: &[u8]) -> Fingerprint {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    Fingerprint(hasher.finish())
}

fn extract_server_facts(
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
                has_definition: server.contains_key("command") || server.contains_key("url"),
            },
        );
    }
    Ok(servers)
}

fn base_views(base: &BTreeMap<String, ServerFacts>) -> Vec<ServerView> {
    base.iter()
        .map(|(name, facts)| ServerView {
            name: name.clone(),
            state: OverrideState::Inherited,
            effective_enabled: facts.enabled.unwrap_or(true),
            inherited_enabled: facts.enabled.unwrap_or(true),
            origin: if facts.has_definition {
                ServerOrigin::Base
            } else {
                ServerOrigin::OverrideOnly
            },
            valid: facts.enabled_is_valid && facts.has_definition,
        })
        .collect()
}

fn merge_server_views(
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
            let base_definition = base_facts.is_some_and(|facts| facts.has_definition);
            let profile_definition = profile_facts.is_some_and(|facts| facts.has_definition);
            let enabled_valid = base_facts.is_none_or(|facts| facts.enabled_is_valid)
                && profile_facts.is_none_or(|facts| facts.enabled_is_valid);
            let origin = if base_definition {
                ServerOrigin::Base
            } else if profile_definition {
                ServerOrigin::Profile
            } else {
                ServerOrigin::OverrideOnly
            };

            ServerView {
                name,
                state,
                effective_enabled,
                inherited_enabled,
                origin,
                valid: enabled_valid && (base_definition || profile_definition),
            }
        })
        .collect()
}

fn set_server_override(
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
        document["mcp_servers"] = Item::Table(Table::new());
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

fn atomic_write(
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

fn profile_name_from_path(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let name = filename.strip_suffix(PROFILE_SUFFIX)?;
    if validate_profile_name(name).is_ok() {
        Some(name.to_owned())
    } else {
        None
    }
}

fn validate_profile_name(name: &str) -> Result<(), StoreError> {
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
fn set_private_file_permissions(file: &File, path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| StoreError::Write {
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File, _path: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        StoreError::Write {
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}

fn sync_parent(path: &Path) {
    if let Ok(directory) = OpenOptions::new().read(true).open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::tempdir;

    use super::{ConfigStore, OverrideState, ServerOrigin, StoreError};

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
}
