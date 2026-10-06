# glint

A tray system monitor for Windows. A tray icon opens a small borderless window
with CPU, memory, GPU, NPU and disk statistics. The window stays open and on
top until it is minimized. The idle cost is near zero.

Home repo: `glint`. Requirement prefix: `SG`.

## Scope

In scope: the five resource rows, the tray icon, the popup window, the minimize
button, disk space, and start with Windows.

Out of scope: per-process lists, temperatures, fan speeds, a taskbar AppBar,
and history beyond 60 seconds.

## Architecture

| Concern | Module |
|---------|--------|
| PDH query and counter wrappers | `src/pdh.rs` |
| `GPU Engine` instance name parse | `src/luid.rs` |
| Adapter discovery through WDDM | `src/wddm.rs` |
| Adapter discovery through DXCore (spike only) | `src/adapters.rs` |
| Metric sampling | `src/sampler.rs` |
| GDI drawing | `src/render.rs` |
| Window, tray icon, interaction | `src/window.rs` |
| Settings persistence | `src/config.rs` |

Probe binaries: `spike` (Spike 1), `wildcard` (PDH wildcard behaviour),
`sample` (console sampler), `cpucheck` (CPU counter semantics).

---

## Requirements

### GLINT-TRAY-TOGGLE: The tray icon shows and hides the window

**Applies to:** glint
**Verification:** manual

A left click on the tray icon shall show the window when it is hidden, and
hide it when it is shown. This holds whether the icon sits on the taskbar or
in the overflow flyout.

One click delivers `WM_LBUTTONUP` and then `NIN_SELECT`, about 5 ms apart.
The window toggles on `NIN_SELECT` (and `NIN_KEYSELECT`) only. Toggling on
both shows the window and hides it again before it can be seen.

**Verification (manual):**

1. Start `glint.exe`.
2. Left click the tray icon. The window appears.
3. Left click the tray icon again. The window disappears.
4. Left click once more. The window appears again.
5. Repeat steps 2 to 4 with the icon in the overflow flyout (the `^`
   chevron).

---

### GLINT-ALWAYS-OPEN: The window stays open and on top

**Applies to:** glint
**Verification:** manual

The window shall stay open and above other windows until the user closes it
with the minimize button, a left click on the tray icon, or **Exit**. Losing
focus shall not hide it. There is no pin control: the window is always pinned.

This replaces GLINT-PIN and GLINT-LIGHT-DISMISS, which are retired. A window
that never hides on focus loss has no use for a pin button or a **Pin** menu
item.

An old `pinned` key in the config file is ignored.

**Verification (manual):**

1. Open the window. Click another application. The window stays open and on
   top.
2. Right click the tray icon. The menu has no **Pin** item, and the header has
   no pin button.
3. Move the tray icon into the overflow flyout (the `^` chevron). Click the
   chevron, then click the glint icon. The window opens and stays open.
4. Left click the tray icon. The window hides.

---

### GLINT-IDLE-COST: A hidden window costs almost nothing

**Applies to:** glint
**Verification:** manual

While the window is hidden, the app shall poll at most once per 10 seconds and
shall not render. While the window is visible, it shall poll once per second.

**Verification (manual):**

1. Start the app and leave the window hidden.
2. In Task Manager, watch the `glint` process for 60 seconds.
3. CPU shall read 0%, and memory shall stay flat.

Measured on 2026-10-05 (AMD Ryzen AI 9 HX PRO 370, Windows 11 26200):

| State | Working set | Private bytes | CPU |
|-------|-------------|---------------|-----|
| Hidden | 14.8 MB | 6.3 MB | 0.000% of one core over 30 s |
| Visible | 20.0 MB | 6.6 MB | 0.391% of one core over 20 s |

Working set includes shared system libraries. Private bytes is the app's own
memory, and it is the number to watch.

---

### GLINT-NPU-OPTIONAL: No NPU, no NPU row

**Applies to:** glint
**Verification:** manual

The NPU row shall be absent when the machine exposes no compute-only adapter.
The window shall be shorter by exactly one row, with no gap and no `--`.

The NPU is found through `D3DKMTEnumAdapters3` with the `IncludeComputeOnly`
filter, and through the `ComputeOnly` flag of `KMTQAITYPE_ADAPTERTYPE`. DXCore
does not find it; see **Why not DXCore** below.

A 0% row has two very different causes: the resource is idle, or the row
matches no counter instance at all. The wiring report separates them, so an
NPU row stuck at 0% cannot hide a wrong LUID.

**Verification (manual):**

1. Run `cargo run --release --bin sample`. It opens with a wiring report:

   ```
   --- wiring ---
     0x00000000_0x00016BCB  GPU row   288 instances  AMD Radeon(TM) 890M Graphics
     0x00000000_0x0001B433  NPU row     4 instances  (no driver name)
     0x00000000_0x0001B3D1  unused    250 instances  (no driver name)
   ```

2. The NPU row shall match at least one instance. A row that matches none
   prints a warning, and that is a defect, not an idle NPU.
3. Open the window. The NPU row is present exactly when the report named one.

Verified under load on 2026-10-05. An NPU workload ran while both this app and
`Get-Counter` sampled the same counters over 14 seconds:

| Independent read | glint |
|------------------|------------|
| 107.3% | 100% |
| 93.1% | 94% |
| 108.8% | 100% |
| 92.2% | 94% |
| 107.9% | 100% |
| 93.6% | 94% |
| 92.5% | 94% |
| 107.0% | 100% |
| 92.3% | 94% |
| 108.6% | 100% |
| 93.4% | 94% |
| 92.4% | 94% |
| 30.8% | 16% |
| 0.0% | 0% |

Where both readers are below 100, they agree within about 2 points, which is
well inside the 5 points the plan asked for. The readings above 100 are
clamped to 100 (GLINT-CLAMP). The last two rows differ because the burst ended
between the two samplers, which take their samples at different instants.

---

### GLINT-ALERT-COLOR: A busy row changes colour

**Applies to:** glint
**Verification:** manual

A row shall turn amber at 80% or more, and red at 95% or more. The colour
applies to the percent text and to the sparkline. The resource label keeps its
own hue, so the row stays identifiable.

**Verification (manual):**

1. Open the window and read the disk space section. A drive above 80% full
   shows an amber bar and amber text.
2. Run a CPU load, such as a build. The CPU row turns amber above 80% and red
   above 95%.

Observed on 2026-10-05: an NPU at 94% drew the percent text and the sparkline
in amber, and the NPU label kept its own hue. A drive at 92% full drew its bar
and its percent in amber in the same window.

A wrong threshold colour is visible on screen in one second, so this stays
`manual`.

---

### GLINT-CPU-METRIC: The CPU percent is measured against the base clock

**Applies to:** glint
**Verification:** manual

The CPU percent shall come from `\Processor Information(_Total)\% Processor
Utility`, which is the counter that Task Manager shows.

That counter measures work done against the **base** clock, not against the
boost ceiling. A faster clock therefore **raises** the percent:

```
% Processor Utility  ~=  % Processor Time  *  % Processor Performance / 100
```

- 50% busy at the base clock reads about 50%.
- 50% busy at twice the base clock reads about 100%, because the core did
  twice the work in the same second.

It does **not** divide by the clock. A reading of 50% at the base clock does
not fall to 25% when the clock doubles. Dividing by the boost ceiling would
give a "headroom used" number instead, which no Windows counter reports, and
which this machine cannot supply: both base-clock sources report 2000 MHz,
and never the 5.1 GHz boost ceiling.

Measured on 2026-10-05 with `cargo run --release --bin cpucheck`:

| State | Time % | Performance % | Utility % | Time x Perf / 100 |
|-------|--------|---------------|-----------|-------------------|
| idle | 15.9 | 187.5 | 36.4 | 29.8 |
| idle | 20.6 | 184.0 | 41.0 | 37.9 |
| loaded | 45.1 | 168.8 | 82.0 | 76.1 |
| loaded | 56.7 | 168.4 | 97.2 | 95.5 |
| loaded | 51.7 | 170.5 | 94.2 | 88.1 |

Utility tracks the product, which is what "measured against the base clock"
predicts.

**Verification (manual):**

1. Open the window beside the Task Manager Performance tab.
2. The CPU percent tracks the Task Manager CPU number at idle and under load.
3. Run `cargo run --release --bin cpucheck` and confirm that the utility
   column tracks the product column.

Cross-checked on 2026-10-05 against `Get-Counter` over an overlapping window:

| Reader | CPU range | Disk range | Clock |
|--------|-----------|------------|-------|
| glint | 7% to 25% | 0% to 3% | 1.29x to 1.78x |
| `Get-Counter` | 9% to 28% | 0.5% to 3.9% | 145% to 200% |

---

### GLINT-CLAMP: Percent values are clamped before display

**Applies to:** glint
**Verification:** manual

Every percent the window shows shall be inside 0 to 100.

`% Processor Utility` exceeds 100 above the base clock (see GLINT-CPU-METRIC),
and a summed `GPU Engine` engine type can exceed 100 across engines. The
sparkline ceiling is a fixed 100, so an unclamped value would draw outside its
box. Task Manager caps the same counter the same way.

Observed on 2026-10-05: an NPU workload drove the raw sum of
`\GPU Engine(*)\Utilization Percentage` for the NPU adapter to 108.8%, and
the window displayed 100%. The sum passes 100 because several processes each
hold a share of one engine.

**Verification (manual):**

1. Run a full CPU load, and an NPU load if the machine has an NPU.
2. No row reads above 100%, and no sparkline draws outside its band.
3. Compare with the raw counter sum, which may read above 100.

---

### GLINT-CPU-BOOST: The CPU row shows the clock and the boost ratio

**Applies to:** glint
**Verification:** manual

The CPU row shall carry a second line with the current clock in GHz and the
ratio to the base clock, as `3.73 GHz  (1.87x base)`.

The clock is the base clock times `% Processor Performance`. That counter must
be read with `PDH_FMT_NOCAP100`: PDH otherwise clamps it to 100, and every
boost ratio collapses to 1.00x.

The base clock comes from the registry value `~MHz` under
`HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0`, and falls back to
`MaxMhz` from `CallNtPowerInformation`.

**Verification (manual):**

1. Open the window beside the Task Manager Performance tab.
2. The GHz readout tracks the Task Manager Speed readout.
3. At idle the ratio is near or below 1.00x; under load it rises above it.

---

### GLINT-GPU-BUSIEST: The GPU percent is the busiest engine type

**Applies to:** glint
**Verification:** manual

The GPU and NPU percent shall be, per adapter, the sum of
`\GPU Engine(*)\Utilization Percentage` over processes for one engine type,
taken for the busiest engine type, then clamped to 100.

This is what Task Manager shows. A sum over all engine types would
double-count a frame that touches 3D and Copy.

**Verification (manual):**

1. Run `cargo run --release --bin sample` and read the wiring report. The GPU
   row shall match many instances, in the hundreds on a busy desktop.
2. Open the window beside the Task Manager Performance tab.
3. Play a video, then run a 3D load. The GPU percent tracks the Task Manager
   GPU number.

Cross-checked on 2026-10-05 against `Get-Counter`, reading the same counters
at the same time. With a video on screen the app read 14% to 22%, and the
engine breakdown was `3D 9.7, Copy 0.8, Video 21.1`. At idle the app read 0%
to 4%, while the independent read summed the busiest engine type to 0.90%.

---

### GLINT-PLACEMENT: The window opens in the lower right, clear of the taskbar

**Applies to:** glint
**Verification:** manual

With no saved position, the window shall open in the lower right corner of the
work area of the monitor that holds the tray icon, 8 logical pixels from the
right edge and 8 above the bottom.

The work area excludes the taskbar wherever the taskbar sits, so the window
never covers it, and a taskbar on the left or the top still gives the right
corner.

The window does not follow the tray icon. It lands in the same place every
time, which is easier to find than a position that moves when the shell
reorders its icons.

The window shall save a position only after the user drags it by its header.
A move the app itself makes shall not save a position.

`WM_EXITSIZEMOVE` also arrives for programmatic moves, such as a
`SetWindowPos` that crosses monitors. Saving on every one of those pins the
window to a place the user never chose. The window therefore saves only while
a drag that the user started is in progress.

**Reset position** in the right click menu clears the saved position.

The user shall be able to drag the window to any position on any monitor,
including monitors with a different display scale. The app shall not move the
window during a drag.

When a drag crosses onto a monitor with a different scale, Windows sends
`WM_DPICHANGED` with a suggested rectangle. The window takes that origin and
rescales. Re-placing the window there instead snaps it back to its saved spot
mid-drag, which is why it once could not reach a monitor with a different
scale.

On open, the window shall use the saved position when any part of its header
lies on a connected monitor, and the default corner otherwise. That covers a
monitor that was unplugged after the drag. The window is measured at the
target monitor's scale before it moves.

When the content changes height (a drive appears or goes away), the window
shall stay where it is. In the default corner it
re-anchors to the corner and grows upward. Anywhere else it keeps its top left
and moves up only as far as it must to keep its bottom above the taskbar.

**Verification (manual):**

1. Delete `%APPDATA%\Glint\config.json` and start the app.
2. Open the window. It sits in the lower right, above the taskbar.
3. Open and close it several times. The config file gains
   no `x` or `y` key.
4. Drag the window by its header, then close it. The config file now holds
   `x` and `y`, and the window reopens there.
5. Choose **Reset position**. The window returns to the lower right.
6. On a desktop with monitors at different scales (for example a laptop panel
   at 150% and an external monitor at 100%), drag the window slowly from one
   to the other and back. It follows the cursor the whole way, rescales as it
   crosses, and never jumps.
7. Drag it onto the second monitor, close it, and reopen it. It reopens there,
   at that monitor's scale.
8. Drag it near the bottom of a monitor and plug in a USB drive. The window
   grows and moves up only enough to stay above the taskbar.

Measured on 2026-10-05, before the move to the lower right: the window opened
8 pixels from the left edge of the work area and 8 pixels above the taskbar.
Not yet re-measured.

---

### GLINT-MINIMIZE: The minimize button hides the window

**Applies to:** glint
**Verification:** manual

The window header shall carry a minimize button at its far right. A click on it shall hide the window.

Minimize is the only control that closes the window from inside it.

The minimize button does not start a drag.

The minimize button shall show a "Minimize to tray" tooltip after the standard
hover delay.

**Verification (manual):**

1. Open the window and click the minimize button. The window hides.
2. Click the tray icon to reopen it.
3. Drag the header anywhere except the two buttons. The window moves.
4. Press and drag on the minimize button. The window does not move.
5. Rest the pointer on the minimize button. A tooltip reads "Minimize to tray".

---

### GLINT-SINGLE-INSTANCE: One instance per session

**Applies to:** glint
**Verification:** manual

A second launch shall exit without adding a second tray icon. The named mutex
is `Local\glint.instance`, so two signed-in users each get their own.

**Verification (manual):**

1. Start `glint.exe`. One icon appears.
2. Start it a second time. The second process exits at once, and the tray
   still shows one icon.

---

### GLINT-TRAY-RESTORE: The icon returns after Explorer restarts

**Applies to:** glint
**Verification:** manual

The app shall add its icon again when Explorer broadcasts `TaskbarCreated`.
A refused `NIM_ADD` shall retry after 2 seconds, because the shell refuses the
add while it is still starting.

**Verification (manual):**

1. Start the app and confirm the icon.
2. End `explorer.exe` in Task Manager, then start it again.
3. The glint icon comes back, and a left click still toggles the window.

---

### GLINT-STARTUP: Start with Windows writes the Run value

**Applies to:** glint
**Verification:** manual

The **Start with Windows** menu item shall add
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\glint` with the
quoted executable path, and shall delete that value when turned off. The menu
item shall carry a check mark while the value exists.

The path is quoted, so a space in it cannot split the command.

**Verification (manual):**

1. Right click the tray icon and choose **Start with Windows**.
2. Read the registry value. It holds the quoted path to the executable.
3. Choose the item again. The value is gone.

---

### GLINT-RESUME: The sampler rebuilds after sleep

**Applies to:** glint
**Verification:** manual

The app shall re-create its PDH query on `PBT_APMRESUMEAUTOMATIC` and
`PBT_APMRESUMESUSPEND`. PDH state does not survive a sleep and resume cycle,
and a stale query returns frozen values with no error.

**Verification (manual):**

1. Open the window and note the numbers.
2. Sleep the machine, then wake it.
3. Open the window. Every row moves again within two seconds.

---

### GLINT-CONFIG-PARSE: Settings survive a restart, and a damaged file does not stop startup

**Applies to:** glint
**Verification:** test

`%APPDATA%\glint\config.json` shall hold the window position. The reader
shall fall back to the defaults for any key that is missing, unknown or
damaged, and shall never fail. The retired `pinned` and `diskExpanded` keys
from older files are ignored.

A position needs both `x` and `y`. A half-written pair shall be dropped, not
half applied, because a window placed at one stored coordinate and one default
coordinate lands somewhere the user never put it.

**Acceptance criteria:**

- `{"x": -1200, "y": 48}` gives `position = Some((-1200, 48))`
- `{}` gives `position = None`
- `{"pinned": true, "diskExpanded": false, "x": -1200, "y": 48}` gives
  `position = Some((-1200, 48))`
- `"not json at all"`, `""` and `{"x":` each give the defaults
- `{"x": 10}` and `{"y": 10}` each give `position = None`
- An unknown key is ignored, and the known keys still read correctly

Tests: `specs/glint/tests/config_parse.rs`

---

### GLINT-LUID-PARSE: The adapter LUID is parsed from a GPU Engine instance name

**Applies to:** glint
**Verification:** test

The app shall parse a `GPU Engine` instance name into a process id, an adapter
LUID, a physical node, an engine index and an engine type. A malformed name
shall return an error, never a wrong LUID.

The name has the form
`pid_13080_luid_0x00000000_0x00016BCB_phys_0_eng_2_engtype_Compute 0`. The
`pid_` part is optional. The engine type runs to the end of the name, so a
type such as `Compute 0` keeps its space and its index.

This is the one requirement that earns a `test`. A parse bug is silent: the
GPU row reads 0% forever, and no human catches it in review. The inputs are
real strings, and the assertion is a concrete LUID pair.

**Acceptance criteria:**

- `luid_0x00000000_0x0000CAFE_phys_0_eng_1_engtype_3D` gives
  high `0x00000000`, low `0x0000CAFE`, phys `0`, eng `1`, engtype `3D`,
  and no process id
- `pid_13080_luid_0x00000000_0x00016BCB_phys_0_eng_1_engtype_Copy` gives
  process id `13080`
- `..._eng_2_engtype_Compute 0` keeps the engtype `Compute 0`, and its engine
  family is `Compute`
- `..._eng_11_engtype_3D` gives eng `11`, not eng `1`
- A trailing `)` from a full counter path is stripped
- Each of these returns an error rather than a LUID: no `luid_` part;
  `luid_0x00000000_phys_0_...` (one half only); `luid_0x_0x0000CAFE_...`
  (no hex digits); `luid_00000000_0x0000CAFE_...` (no `0x` prefix); no
  `engtype_` part; an empty engtype; no `phys_` part

Tests: `specs/glint/tests/luid_parse.rs`

---

## Design notes

### Why not DXCore for the NPU

The plan assumed DXCore would find the NPU. Spike 1 disproved that on an AMD
Ryzen AI 9 HX PRO 370:

| API | Adapters reported |
|-----|-------------------|
| DXCore, `D3D12_GRAPHICS` | Radeon 890M, Microsoft Basic Render Driver |
| DXCore, `GENERIC_ML` and `D3D12_CORE_COMPUTE` | none |
| `D3DKMTEnumAdapters2` | Radeon 890M, Basic Render Driver |
| `D3DKMTEnumAdapters3` with `IncludeComputeOnly` | all three, plus the NPU |

The XDNA2 NPU driver publishes no Direct3D 12 device, so no DXCore attribute
matches it. `shared/d3dkmthk.h` states the cause for `EnumAdapters2`:
"ComputeOnly adapters are left out of the default enumeration, to avoid
breaking applications."

`src/adapters.rs` keeps the DXCore path, because the spike prints both lists
side by side. `src/wddm.rs` is what the app uses.

### Why GDI, and not Direct2D

A Direct2D render target creates a D3D device. That costs RAM, and it holds
the GPU awake. Five rows and five sparklines need no GPU. GDI draws them in
well under a millisecond, and private memory stays near 6.5 MB.

The window paints an opaque background. A DWM Mica or acrylic backdrop needs
the window to leave pixels unpainted, which a double-buffered `BitBlt` cannot
do. The window takes DWM rounded corners and dark mode, which is the part of
the plan that GDI and DWM can both honour.

### The PDH wildcard re-expands by itself

The plan called for re-adding the `GPU Engine(*)` counter every 30 seconds.
`cargo run --release --bin wildcard` proves that is unnecessary: a counter
added before a process started reports all 33 of its instances. A re-add would
cost one bad sample for nothing. The sleep and resume path still rebuilds the
whole query (GLINT-RESUME).

### Measured cost of one tick

| Measure | Result |
|---------|--------|
| `GPU Engine` instances | about 500 at idle |
| `PdhCollectQueryData` | 0.25 ms mean |
| `PdhGetFormattedCounterArrayW` plus parse | 0.18 ms mean |
| Total per tick | 0.43 ms mean, 0.81 ms worst |

The plan allowed 2 ms, so there is no worker thread and no second poll rate.
Reused buffers keep a steady tick at zero allocations.

## Open items

None. The NPU exit criterion from the plan is closed; see GLINT-NPU-OPTIONAL for
the measured comparison under load.
