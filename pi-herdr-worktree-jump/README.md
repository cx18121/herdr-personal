# Pi Herdr Worktree Jump

Relocates the active Pi session into a Worktrunk-managed worktree while preserving its conversation. It is based on Can Celik's MIT-licensed [`@ogulcancelik/pi-herdr-worktree-jump`](https://github.com/ogulcancelik/pi-extensions/tree/main/packages/pi-herdr-worktree-jump).

## Behavior

For a new destination, the tool:

1. Resolves the repository's source checkout through Herdr.
2. Creates the branch and checkout with `wt switch --create`, including Worktrunk lifecycle hooks.
3. Uses Worktrunk's local default branch when no base is supplied. Keep that branch current with its upstream before jumping.
4. Opens the checkout as a Herdr worktree workspace.
5. Forks the persisted Pi session into the checkout and starts it in the new pane.
6. Shuts down the old Pi process and closes its pane.

An explicit `base` overrides Worktrunk's local default branch. Returning to the main checkout continues to use Herdr directly because it creates no checkout.

Worktrunk project hooks must be approved before a non-interactive Pi jump. Run `wt config approvals add` from the repository when its hook commands are new or changed.

## Install

```bash
pi install ~/Projects/personal/herdr-personal/pi-herdr-worktree-jump
```

Restart Pi or run `/reload` after changing the extension.

## Requirements

- Pi 0.80 or newer
- Herdr 0.8 or newer
- Worktrunk on `PATH`
- Pi running in a Herdr pane with a persisted session
