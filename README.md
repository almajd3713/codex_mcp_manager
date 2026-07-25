# codex-mcp

`codex-mcp` is a small terminal UI for organizing Codex CLI MCP servers into
native Codex configuration profiles and launching Codex with one of those
profiles.

It does not introduce another profile format. Codex loads its base
`$CODEX_HOME/config.toml` and overlays
`$CODEX_HOME/<profile>.config.toml`; this tool reads the same files and only
edits each MCP server's profile-level `enabled` value. See the official
[Codex profile documentation](https://learn.chatgpt.com/docs/config-file/config-advanced#profiles)
and [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference#configtoml).

## Install

Rust 1.85 or newer is required.

```sh
cargo install --path .
```

Ensure both `codex-mcp` and `codex` are available on `PATH`.

## Usage

```sh
codex-mcp
codex-mcp -- -C /path/to/project
codex-mcp --codex-home /path/to/another/codex-home
codex-mcp --codex-bin /path/to/codex -- "start with this prompt"
```

Arguments after `--` are passed to Codex unchanged. Do not pass `-p` or
`--profile`; the selected row determines the profile.

The `Base` row launches Codex without a profile. Selecting a named profile
launches:

```sh
codex --profile <name> <forwarded arguments>
```

The current directory and environment are preserved.

## Keys

Profile picker:

- `Enter`: launch the selected row
- `n`: create a profile
- `e`: edit MCP states
- `r`: rename a profile
- `d`: move a profile to the recovery directory
- `j`/`k` or arrow keys: navigate
- `q` or `Esc`: quit

Profile editor:

- `Space`: cycle `Inherited`, `Enabled`, and `Disabled`
- `s`: save
- `Esc`: discard the in-memory changes

`Inherited` means the profile has no `enabled` override for that server. The
effective state then comes from `config.toml`; if neither layer contains an
`enabled` value, Codex's default is enabled.

## Safety and scope

- Base MCP definitions are read-only. Commands, arguments, URLs, headers, and
  environment values are never rendered by the TUI.
- Profile changes preserve unrelated settings and TOML comments.
- Saves use a same-directory temporary file and atomic rename. A save is
  rejected if another process changed the profile after it was opened.
- New profile files use user-only permissions on Unix.
- Deleted profiles are moved to
  `$CODEX_HOME/.codex-mcp-trash/`, not permanently removed.
- Malformed profiles are shown as invalid and cannot be edited or launched by
  the manager until repaired externally.

Adding or removing MCP server definitions and performing OAuth login/logout
remain the responsibility of `codex mcp`. Plugin-bundled MCP servers and native
Windows are outside the initial release.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

