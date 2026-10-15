# First-run "Install for me"

Written for: Cyrus, reviewing before implementation.

## Behaviour

On launch, if `glint.exe` is not running from `%LOCALAPPDATA%\Programs\Glint\`, glint asks once whether to install itself there.

- **Install for me**: copy the exe to `%LOCALAPPDATA%\Programs\Glint\glint.exe`, start that copy, and exit.
- **No**: write `"installPrompt": "declined"` to `config.json`. The prompt never appears again.
- **Tray menu**: an **Install for me** item stays available whenever the running exe is not the installed copy, whether or not the user declined. It disappears once running from the install folder.

## Spec impact

- **New** `GLINT-INSTALL`: Verification `manual`. Procedure covers: prompt on first run, decline persists, menu item still works after declining, install copies and relaunches, menu item gone when run from the install folder.
- **Modified** `GLINT-CONFIG-PARSE` (`test`): adds the `installPrompt` key. A missing or damaged value means "not yet asked". Acceptance criteria to add:
  - `{}` gives `install_declined = false`
  - `{"installPrompt": "declined"}` gives `true`
  - `{"installPrompt": }` and `{"installPrompt": 5}` give `false`
  - the key does not disturb the position or the visibility flags
- **Touches, not modified**: `GLINT-SINGLE-INSTANCE` and `GLINT-STARTUP`. See the risks below.

## Design

**Detecting "installed".** Compare the parent directory of `current_exe()` to the install directory, case-insensitively, after canonicalizing both. A copy anywhere else counts as not installed.

**The prompt.** `MessageBoxW` with `MB_YESNOCANCEL`. The body says what each button does:
- **Yes**: install.
- **No**: don't ask again (persists `declined`).
- **Cancel**, X or Esc: not now, asks again next launch. Only an explicit No persists, so a stray Esc cannot silently disable the offer.
- No manifest, no new `windows` features beyond `Win32_UI_WindowsAndMessaging`, which is already enabled.

**Where it runs.** In `main()` after the mutex is claimed, before `window::run()`. The tray icon does not exist yet, so the dialog is the only UI. Release builds only (`!cfg!(debug_assertions)`), so `cargo run` does not nag. The menu item stays in debug builds for testing.

**The install step**, shared by the dialog and the menu:
1. `create_dir_all` the install directory.
2. Copy `current_exe()` to `glint.exe` there. If the copy fails because the file is locked (an installed copy is already running), tell the user and carry on from the current exe. Do not kill the other process.
3. If the `Run` value exists, rewrite it to the installed path (reuse `set_startup_enabled(true)`, which already writes `current_exe()`. This needs a small refactor to take a path). Do **not** turn startup on if it was off.
4. Drop the single-instance mutex, then `CreateProcessW` the installed exe, then exit. The mutex has to go first or the child sees "already running" and quits. If the spawn fails, keep running and report it.

From the menu, step 4 needs a way to release the mutex from `window.rs`. Options: a `Drop`-able guard passed into `run()`, or a process-wide static handle. Static handle is smaller.

**Config.** `Config` gains `install_declined: bool`, serialized as `"installPrompt": "declined"` only when true, and omitted otherwise. The hand-written reader gets a `read_str` beside `read_bool`.

**Menu.** New `MENU_INSTALL = 120`. The item goes above "Start with Windows", and only exists when not installed. Clicking it never touches `install_declined`.

## Risks and things I'd push on

- **Updates.** Downloading a newer glint.exe and running it re-triggers the prompt (not installed). The user would have declined or accepted once, so for an updating user the flow is: accept again and overwrite the old copy. That is fine as long as the locked-file case is handled: the old copy is running, so the copy fails. Best handling is to message "Exit the running glint first" rather than try to be clever. Real self-update is out of scope.
- **No uninstaller.** Install adds nothing beyond the one exe and the config, so uninstalling is deleting the folder (and toggling Start with Windows off). Not adding an Add/Remove Programs entry or Start Menu shortcut, to keep "light". Say so if you want a shortcut.
- **Declined users on the installed path.** A user who declined and then manually puts the exe in the install folder just stops seeing the item. Fine.
- **SmartScreen / AV.** A program copying itself and relaunching can trip heuristics. Small risk; unsigned exe already has SmartScreen friction.

## Tests

- `config_parse.rs`: the four new criteria above.
- Install, detection and dialog are manual (UI, filesystem, process spawn). No stubs.

## Order of work

1. Config key and tests (red first).
2. `install.rs`: `install_dir()`, `is_installed()`, `install_self()`.
3. Dialog (MessageBoxW).
4. Menu item and mutex release.
5. Spec: add `GLINT-INSTALL`, extend `GLINT-CONFIG-PARSE`, update Scope and Architecture table; archive this plan.

## Decisions

1. Install to `%LOCALAPPDATA%\Programs\Glint\glint.exe`.
2. Install does not touch Start with Windows beyond re-pointing an existing Run value.
3. `MessageBoxW`, no manifest.
