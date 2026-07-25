use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchTarget {
    Base,
    Profile(String),
}

pub fn command_args(target: &LaunchTarget, forwarded: &[OsString]) -> Vec<OsString> {
    let mut args = Vec::with_capacity(forwarded.len() + 2);
    if let LaunchTarget::Profile(name) = target {
        args.push(OsString::from("--profile"));
        args.push(OsString::from(name));
    }
    args.extend_from_slice(forwarded);
    args
}

pub fn launch(
    codex_bin: &OsStr,
    codex_home: &Path,
    target: &LaunchTarget,
    forwarded: &[OsString],
) -> io::Error {
    let command = build_command(codex_bin, codex_home, target, forwarded);
    launch_command(command)
}

fn build_command(
    codex_bin: &OsStr,
    codex_home: &Path,
    target: &LaunchTarget,
    forwarded: &[OsString],
) -> Command {
    let mut command = Command::new(codex_bin);
    command
        .args(command_args(target, forwarded))
        .env("CODEX_HOME", codex_home);
    command
}

#[cfg(unix)]
fn launch_command(mut command: Command) -> io::Error {
    use std::os::unix::process::CommandExt;

    command.exec()
}

#[cfg(not(unix))]
fn launch_command(mut command: Command) -> io::Error {
    match command.status() {
        Ok(_) => io::Error::other("Codex exited"),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::process::Command;

    use tempfile::tempdir;

    use super::{LaunchTarget, build_command, command_args};

    #[test]
    fn base_forwards_arguments_unchanged() {
        let forwarded = vec![OsString::from("-C"), OsString::from("/tmp/work")];
        assert_eq!(command_args(&LaunchTarget::Base, &forwarded), forwarded);
    }

    #[test]
    fn profile_is_inserted_before_forwarded_arguments() {
        let forwarded = vec![OsString::from("review this")];
        assert_eq!(
            command_args(&LaunchTarget::Profile("work".to_owned()), &forwarded),
            vec![
                OsString::from("--profile"),
                OsString::from("work"),
                OsString::from("review this")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn fake_codex_receives_home_directory_and_arguments() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let fake_codex = temp.path().join("fake-codex");
        fs::write(
            &fake_codex,
            "#!/bin/sh\nprintf '%s\\n' \"$CODEX_HOME\" \"$PWD\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o700)).unwrap();

        let forwarded = vec![OsString::from("-C"), OsString::from("/tmp/project")];
        let mut command = build_command(
            fake_codex.as_os_str(),
            temp.path(),
            &LaunchTarget::Profile("work".to_owned()),
            &forwarded,
        );
        command.current_dir(&workspace);
        let output = Command::output(&mut command).unwrap();
        let lines = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        assert_eq!(lines[0], temp.path().display().to_string());
        assert_eq!(lines[1], workspace.display().to_string());
        assert_eq!(&lines[2..], ["--profile", "work", "-C", "/tmp/project"]);
    }
}
