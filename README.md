# herdr-personal

Portable personal Herdr configuration, commands, and plugins for macOS and Linux.

The top-level machine bootstrap in `cx18121/dotfiles` invokes this package. To apply it directly inside an already configured mise environment:

```bash
mise run bootstrap
mise run check
```

## What it installs

- `config/config.toml` as `~/.config/herdr/config.toml`
- `cmd+w` pane close with history and `cmd+shift+t` reopen
- A stable `~/.local/bin/wt` launcher for Worktrunk 0.77.0
- Pinned GitHub plugins from `plugins.tsv`
- The local Arrange plugin from `layouts/`
- Worktrunk plugin settings

The portable plugin set contains GitHub PR status, automatic rename, file viewer, and Worktrunk. Collie remains machine-specific and is not installed by this package.

## Source layout

- `config/` contains the complete portable Herdr configuration.
- `plugins.tsv` pins GitHub plugin sources and tags.
- `pane-history.mjs` implements close and reopen behavior.
- `bin/herdr-pane-history` gives Herdr a stable launcher whose Node runtime comes from mise.
- `layouts/` contains the Arrange plugin and its Rust implementation.
- `pi-herdr-worktree-jump/` contains Pi session relocation support.
- `scripts/bootstrap` installs or updates the package idempotently.
- `scripts/check` verifies the resulting runtime surface.

Herdr runtime state, logs, sockets, sessions, pane history, credentials, and plugin state are not tracked.
