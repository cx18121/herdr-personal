# Arrange

Arrange changes the existing pane layout in the current Herdr tab without restarting their processes or clearing scrollback. Current Herdr builds apply full layout changes atomically, so the tab has only one resize and repaint. Older builds use a safe temporary-tab fallback.

## Shortcuts

- `⌘⌥L` opens the visual layout picker.
- `⌘⌥Enter` expands the focused pane.
- `⌘⌥B` balances the closest split.
- `⌘⌥R` rotates the closest split.
- `⌘⌥Z` undoes the latest Arrange change in the current tab.

The picker supports two to four panes and opens on Actions with Rearrange selected. Press Tab or the left and right arrow keys to switch between Actions and Layouts. The Layouts tab opens on the preset that matches the current arrangement. Applied changes update the background while the picker stays open. Press Escape when you are finished.

## Layouts

- Columns
- Rows
- Grid for four panes
- Focused pane on the left, right, top, or bottom

Focused layouts give the focused pane two thirds of the available space. The other panes divide the remaining space evenly.

## Rearranging

The preview is ready to rearrange as soon as the picker opens:

- Pane numbers stay tied to pane IDs while the picker is open, so they move with the panes.
- Hover a numbered pane to see that it can be dragged.
- Drag a numbered pane near another pane's left, right, top, or bottom edge to dock it there.
- Drop it in the center to swap the two pane positions.
- The highlighted area shows the active drop position.
- Release outside a pane or press Escape to cancel.

For keyboard rearranging, press Enter. Use the arrow keys to choose a pane, Space to pick it up, the arrow keys to choose a target, Tab to choose its position, and Space again to apply. Escape first puts down a picked-up pane, then returns to the action list.

The Actions tab contains Rearrange, Expand, Balance, Rotate, and Undo. The Layouts tab contains the equal and focused presets. Selecting an action or layout updates the preview before it is applied.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
herdr plugin link ~/.config/herdr/plugins/local/layouts
```
