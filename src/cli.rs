use std::ffi::OsString;
use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "codex-mcp",
    version,
    about = "Select an MCP-focused Codex configuration profile and launch Codex"
)]
pub struct Cli {
    /// Override CODEX_HOME (defaults to $CODEX_HOME, then ~/.codex).
    #[arg(long, value_name = "PATH")]
    pub codex_home: Option<PathBuf>,

    /// Codex executable to launch.
    #[arg(long, value_name = "PATH", default_value = "codex")]
    pub codex_bin: OsString,

    /// Arguments forwarded to Codex after selecting a profile.
    #[arg(last = true, value_name = "CODEX_ARGS")]
    pub codex_args: Vec<OsString>,
}

impl Cli {
    pub fn resolve_codex_home(&self) -> Result<PathBuf, String> {
        if let Some(path) = &self.codex_home {
            return Ok(path.clone());
        }

        if let Some(path) = std::env::var_os("CODEX_HOME") {
            return Ok(PathBuf::from(path));
        }

        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".codex"))
            .ok_or_else(|| {
                "could not determine Codex home; pass --codex-home or set CODEX_HOME".to_owned()
            })
    }

    pub fn validate_forwarded_args(&self) -> Result<(), String> {
        for arg in &self.codex_args {
            let arg = arg.to_string_lossy();
            if arg == "-p" || arg == "--profile" || arg.starts_with("--profile=") {
                return Err(
                    "do not pass -p/--profile after --; the TUI owns profile selection".to_owned(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::Cli;

    fn cli(args: &[&str]) -> Cli {
        Cli {
            codex_home: None,
            codex_bin: OsString::from("codex"),
            codex_args: args.iter().map(OsString::from).collect(),
        }
    }

    #[test]
    fn rejects_forwarded_profile_flags() {
        for args in [
            vec!["--profile", "work"],
            vec!["-p", "work"],
            vec!["--profile=work"],
        ] {
            assert!(cli(&args).validate_forwarded_args().is_err());
        }
    }

    #[test]
    fn accepts_other_forwarded_arguments() {
        assert!(
            cli(&["-C", "/tmp/project", "review this"])
                .validate_forwarded_args()
                .is_ok()
        );
    }
}
