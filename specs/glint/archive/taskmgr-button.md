# Task Manager button next to minimize

**Outcome (2026-10-12, built):** shipped as GLINT-TASKMGR. `ShellExecuteW` on
`taskmgr.exe` raised no UAC prompt on this machine, so the `SendInput` fallback
was not needed. `open_url` became `shell_open`.

A second header button, left of minimize, that opens Windows Task Manager.
Sorting it by memory is dropped for now (no supported way; see the earlier
discussion). This is only the launcher.

## Spec impact

**New requirement: GLINT-TASKMGR** (manual)

- The header carries a button immediately left of the minimize button, same
  size and style, drawn as three ascending bars.
- A click starts Task Manager. The window stays open.
- The button does not start a drag.
- Tooltip "Open Task Manager" after the standard hover delay.

**Modified: GLINT-MINIMIZE.** Its drag step already says "except the two
buttons", so only the scope line and GLINT-FIT's header mention need checking.
No behaviour change to minimize.

## Changes

| File | Change |
|---|---|
| `src/render.rs` | `Layout.taskmgr_rect` = minimize rect shifted left by `button + pad / 2`. `draw_header` ends the "Glint" text at `taskmgr_rect.left`. New `draw_taskmgr` with three filled bars, like `draw_minimize`. |
| `src/window.rs` | `TIP_TASKMGR = 2` added to both tooltip loops (`:415`, `:435`). `TTN_GETDISPINFOW` (`:874`) picks the text from `header.hdr.idFrom` (today it always says "Minimize to tray"). `on_click` (`:584`) launches Task Manager. `WM_LBUTTONDOWN` (`:839`) excludes the new rect from the drag. |

The launch reuses the `ShellExecuteW` call that already opens the tip link
(`window.rs:967`), so no new `windows` feature and no new dependency. glint's
core stays as light as it is.

## Open question to settle first

`taskmgr.exe` has an elevation manifest, so `ShellExecuteW("open", "taskmgr.exe")`
may raise a UAC prompt on every click, where Ctrl+Shift+Esc does not. I haven't
checked. Step one of implementation is to try it. If it prompts, the options are:

1. Accept the prompt.
2. Launch through `explorer.exe shell:::{...}` or send the Ctrl+Shift+Esc
   hotkey with `SendInput`, which Windows treats as the user's own request.

I would try option 2 only if the prompt shows up.

## Verification (manual, goes in the spec)

1. Open the window. A bar-chart button sits left of minimize.
2. Click it. Task Manager opens, and the glint window stays open.
3. Press and drag on it. The window does not move.
4. Rest the pointer on it. The tooltip reads "Open Task Manager". The minimize
   tooltip still reads "Minimize to tray".
5. Move the window to a monitor at a different scale. Both buttons rescale and
   the "Glint" label is not clipped.

Then: add GLINT-TASKMGR to `spec.md`, move this plan to `archive/`.
