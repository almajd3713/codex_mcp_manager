use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::NamedTempFile;
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

mod document;
mod model;

#[cfg(test)]
mod tests;

pub use document::{validate_new_server, validate_server_name};
pub use model::{
    Catalog, Fingerprint, NewServer, OverrideState, ProfileRecord, ServerDefinition, ServerOrigin,
    ServerView, StoreError, TransportKind,
};

use document::*;

const PROFILE_SUFFIX: &str = ".config.toml";
const TRASH_DIR: &str = ".codex-mcp-trash";

#[derive(Clone, Debug)]
struct ServerFacts {
    enabled: Option<bool>,
    enabled_is_valid: bool,
    transport: TransportKind,
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
        let base_fingerprint = base_doc.fingerprint;

        let mut profiles = Vec::new();
        let entries = match fs::read_dir(&self.home) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Catalog {
                    base_fingerprint,
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
            base_fingerprint,
            base_servers,
            profiles,
        })
    }

    pub fn add_server_and_assign(
        &self,
        server: &NewServer,
        expected_base: Fingerprint,
        assignments: &[(String, Fingerprint)],
    ) -> Result<Vec<String>, StoreError> {
        validate_new_server(server)?;
        let base_path = self.home.join("config.toml");
        let loaded = read_optional_document(&base_path)?;
        if loaded.fingerprint != expected_base {
            return Err(StoreError::BaseConcurrentChange);
        }
        let existing = extract_server_facts(&loaded.document, &base_path)?;
        if existing.contains_key(&server.name) {
            return Err(StoreError::ServerExists(server.name.clone()));
        }

        let home_already_existed = self.home.exists();
        fs::create_dir_all(&self.home).map_err(|source| StoreError::Write {
            path: self.home.clone(),
            source,
        })?;
        if !home_already_existed {
            set_private_dir_permissions(&self.home)?;
        }

        let mut document = loaded.document;
        insert_server_definition(&mut document, server, &base_path)?;
        let rendered = document.to_string();

        let current = read_optional_document(&base_path)?;
        if current.fingerprint != expected_base {
            return Err(StoreError::BaseConcurrentChange);
        }
        let permissions = fs::metadata(&base_path)
            .ok()
            .map(|metadata| metadata.permissions());
        atomic_write(&base_path, &rendered, permissions)?;

        let mut failed_profiles = Vec::new();
        for (profile_name, fingerprint) in assignments {
            let states = BTreeMap::from([(server.name.clone(), OverrideState::Enabled)]);
            if self
                .save_profile_states(profile_name, *fingerprint, &states)
                .is_err()
            {
                failed_profiles.push(profile_name.clone());
            }
        }
        Ok(failed_profiles)
    }

    pub fn inspect_base_source(&self) -> Result<String, StoreError> {
        read_optional_source(&self.home.join("config.toml"))
    }

    pub fn inspect_profile_source(&self, name: &str) -> Result<String, StoreError> {
        validate_profile_name(name)?;
        let path = self.profile_path(name);
        fs::read_to_string(&path).map_err(|source| StoreError::Read { path, source })
    }

    pub fn inspect_server_source(&self, name: &str) -> Result<String, StoreError> {
        validate_server_name(name)?;
        let path = self.home.join("config.toml");
        let loaded = read_optional_document(&path)?;
        let Some(mcp_servers) = loaded
            .document
            .get("mcp_servers")
            .and_then(Item::as_table_like)
        else {
            return Err(StoreError::ServerMissing(name.to_owned()));
        };
        let Some(server) = mcp_servers.get(name).cloned() else {
            return Err(StoreError::ServerMissing(name.to_owned()));
        };

        let mut document = DocumentMut::new();
        let mut table = Table::new();
        table.set_implicit(true);
        table.insert(name, server);
        document["mcp_servers"] = Item::Table(table);
        Ok(document.to_string())
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
