

My personal Herdr plugins and pane history command

## Tools and files

- [`pane-history.mjs`](pane-history.mjs) adds close and reopen behavior for panes. It remembers the pane directory and tab, and it restores the previous Pi session when one is available.
- [`herdr-update`](herdr-update) backs the `herdr update` command. It syncs my fork, builds it, moves running panes and agents to the new build, and then rebuilds the matching binary on the personal cloud host.
- [`herdr-worktrunk-picker`](herdr-worktrunk-picker) opens the Worktrunk picker in a popup pane for the current pane's repository, and refreshes the repository in the background at most once every five minutes.
- [`layouts/herdr-plugin.toml`](layouts/herdr-plugin.toml) registers the Arrange actions, shortcuts, and popup picker with Herdr.
- [`layouts/src/main.rs`](layouts/src/main.rs) reads the requested Arrange action and runs it against the active Herdr pane.
- [`layouts/src/herdr.rs`](layouts/src/herdr.rs) sends commands to the Herdr server and converts its layout responses into local Rust types.
- [`layouts/src/layout.rs`](layouts/src/layout.rs) defines pane layout trees and the operations used to inspect or rearrange them.
- [`layouts/src/operations.rs`](layouts/src/operations.rs) implements expand, balance, rotate, undo, pane rearranging, and the preset layouts.
- [`layouts/src/picker.rs`](layouts/src/picker.rs) implements the keyboard and mouse interface for previewing and applying layout changes.
- [`layouts/src/state.rs`](layouts/src/state.rs) stores one undo record for each tab in the Herdr plugin state directory.
- [`layouts/src/error.rs`](layouts/src/error.rs) defines the errors shown by the Arrange command.
- [`layouts/README.md`](layouts/README.md) explains the Arrange shortcuts, layouts, drag behavior, and keyboard controls.

## Origins

- [`pane-history.mjs`](pane-history.mjs) was built around Herdr's command line interface and agent session support.
- [`layouts`](layouts/README.md) was built against Herdr's [plugin system](https://github.com/cx18121/herdr/blob/master/docs/next/website/src/content/docs/plugins.mdx) and [socket layout API](https://github.com/cx18121/herdr/blob/master/docs/next/website/src/content/docs/socket-api.mdx).

## Build and link Arrange

```bash
cd ~/Projects/personal/herdr-personal/layouts
cargo build --release
herdr plugin link "$PWD"
```

## Install the commands

Both commands run from `~/.local/libexec`, so link them there:

```bash
ln -s ~/Projects/personal/herdr-personal/herdr-update ~/.local/libexec/herdr-update
ln -s ~/Projects/personal/herdr-personal/herdr-worktrunk-picker ~/.local/libexec/herdr-worktrunk-picker
```

The picker is bound to `cmd+shift+o` and `cmd+shift+r` in `~/.config/herdr/config.toml` and needs `jq` plus Worktrunk installed through mise.

Building requires Zig 0.15.2 alongside the Rust toolchain.

The last step calls `sync-herdr.sh` in the `personal-cloud` repository, because a Herdr client and server must speak the same wire protocol and no published release matches this fork. It is skipped when that repository is absent. A remote failure never fails the local update, and `HERDR_UPDATE_SKIP_REMOTE=1` turns the step off.

## Configure pane history

Add commands like these to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "cmd+w"
type = "shell"
command = "$HOME/Projects/personal/herdr-personal/pane-history.mjs close"
description = "close pane and remember it"

[[keys.command]]
key = "cmd+shift+t"
type = "shell"
command = "$HOME/Projects/personal/herdr-personal/pane-history.mjs reopen"
description = "reopen last closed pane"
```
