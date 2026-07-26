use std::io;
use std::path::PathBuf;

use thiserror::Error;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportKind {
    Stdio,
    Http,
    Invalid,
}

#[derive(Clone, Debug)]
pub struct ServerView {
    pub name: String,
    pub state: OverrideState,
    pub effective_enabled: bool,
    pub inherited_enabled: bool,
    pub origin: ServerOrigin,
    pub transport: TransportKind,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fingerprint(pub(super) u64);

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
    pub base_fingerprint: Fingerprint,
    pub base_servers: Vec<ServerView>,
    pub profiles: Vec<ProfileRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerDefinition {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        env_vars: Vec<String>,
    },
    Http {
        url: String,
        bearer_token_env_var: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewServer {
    pub name: String,
    pub definition: ServerDefinition,
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
    #[error("base config changed on disk; reload before saving")]
    BaseConcurrentChange,
    #[error("MCP server name must contain only letters, numbers, hyphens, and underscores")]
    InvalidServerName,
    #[error("MCP server '{0}' already exists in the base config")]
    ServerExists(String),
    #[error("MCP server '{0}' does not exist in the base config")]
    ServerMissing(String),
    #[error("invalid MCP server: {0}")]
    InvalidServer(String),
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}
