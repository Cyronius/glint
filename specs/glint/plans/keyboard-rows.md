# Addons, and the first one: glint's bars on a Keychron's key rows

Written for: Cyrus, reviewing before implementation.

Two pieces of work:

1. **An addon host in glint.** glint can start optional companion programs
   and hand them its live readings. The keyboard comes first, and later ones
   might drive a small LCD or something else.
2. **`glint-keys`, the first addon.** Each tray strip becomes a row of keys on
   a Keychron keyboard. CPU fills the top row left to right, memory the next,
   and so on down. Lit keys turn amber at 80% and red at 95%.

## The constraint that shapes everything

glint stays as it is. Every addon is a separate executable that runs only while
the user has it switched on. glint itself gains no device code and no new
dependency, whatever the addons do.

| | No addons installed | Installed, all off | One or more on |
|---|---|---|---|
| Menu | unchanged | one checkable item per addon | checked |
| Extra processes | none | none | one per addon that is on |
| glint's per-tick cost | one `bool` test | one `bool` test | one copy of about 200 bytes, plus one `SetEvent` per running addon |
| Device code loaded | never | never | only inside the addon processes |

Budget for glint itself: `glint.exe` grows by no more than 4 KB, and the
GLINT-IDLE-COST numbers do not move while every addon is off. Both are measured
and recorded in the spec.

### Alternatives rejected

- **A DLL per addon.** It runs inside glint, so a hang in a USB or serial write
  hangs the tray icon. Unloading it safely while its thread runs is also hard.
- **Each addon samples for itself.** Every addon would repeat the PDH query,
  and each would trail the tray icon by up to a sample.
- **Data over stdin pipes.** A stuck addon eventually fills its pipe, and then
  glint's `WriteFile` blocks the UI thread.
- **Features built into glint.exe.** The keyboard code alone was estimated at
  15 to 30 KB plus a worker thread. Each new device type would add more.

---

# Part 1: the addon host

## Finding addons

An addon is any `glint-<name>.exe` in the same folder as `glint.exe`.

- The menu label comes from the exe's `FileDescription` version resource, for
  example "Keyboard lights". When that resource is missing, the label falls
  back to `<name>`.
- glint lists the folder when the menu opens, not on every tick.
- glint needs no new release to recognise a new addon. Dropping the exe beside
  it is enough.

The menu gains one group, shown only when at least one addon exists:

```
  Hide
  ---------------------------
  Start with Windows
  Reset position
  ---------------------------
  Keyboard lights          v      <- one item per glint-*.exe found
  LCD panel                       <- (a future addon)
  ---------------------------
  ♥  Like this? Tip my local NPR station
  Exit
```

The config stores one key per addon, `"addon.<name>": true`. A key that is
missing or damaged means **off**, unlike the visibility flags: a corrupt file
should never take over someone's keyboard.

## The feed

glint publishes one read-only block of shared memory,
`Local\glint.feed.<pid>`, which every addon maps. The block is created when the
first addon starts and closed when the last one stops. With every addon off,
nothing is created.

```rust
#[repr(C)]
pub struct Feed {
    version: u32,          // layout version; an addon refuses a version it does not know
    seq: AtomicU32,        // seqlock: odd while glint writes
    // bars, in tray icon order: what readings() returns
    bar_count: u32,
    bar_kind: [u8; 6],     // Cpu, Memory, Gpu, Npu, Disk, Network (network-kpi.md)
    bar_percent: [f32; 6],
    // the rest of Metrics, for addons that show more than bars
    cpu_ghz: f32,
    cpu_ratio: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    volume_count: u32,
    volumes: [Volume; 8],  // a 3-character label, used bytes, total bytes
}
```

- **The bars come from `trayicon::readings()`**, the list the icon and its
  hover text already share. No addon can disagree with the icon about which
  bars exist or what order they are in.
- **A seqlock**, because the block is larger than one atomic. glint sets `seq`
  odd, writes, then sets it even. A reader copies the block and retries if
  `seq` changed or was odd. glint is the only writer and never waits on a
  reader.
- **The layout lives in the `glint` lib** (`src/feed.rs`), so the host and
  every addon compile against one definition. `version` covers the case where
  an addon is built from a different glint version than the glint running it.
- History (the sparkline minute) is left out. An addon that wants a trend keeps
  its own; adding it to the block would grow it about thirty times for nothing
  the first addon needs.

## Lifecycle

Each running addon gets two auto-reset events of its own:
`Local\glint.feed.<pid>.<name>.tick` and `….stop`.

- **Start:** glint starts `glint-<name>.exe --parent <pid>` with
  `CREATE_NO_WINDOW`. It does this at launch for each addon that is on, and
  when the user ticks an item.
- **Each tick:** after `update_tray()`, glint writes the feed and signals each
  running addon's tick event.
- **Stop:** when the user unticks the item, or on `WM_DESTROY`, glint signals
  the addon's stop event. It waits up to 1 s for the process to exit, then calls
  `TerminateProcess`.
- **Restart:** an addon that died while it is on is restarted on a later tick,
  at most once per 60 s per addon, so a crash loop costs nothing.
- **glint exits:** every addon holds glint's process handle. If glint exits
  for any reason, crash included, each addon cleans up and exits.

The contract an addon must follow goes into the spec, so a future addon can be
written from the spec alone:

- Map the feed and check `version`.
- Wait on tick, stop and the parent process.
- Clean up on stop or parent exit.
- Never block glint.

---

# Part 2: glint-keys

## What the firmware offers

These details come from reading Keychron's QMK fork (`Keychron/qmk_firmware`,
branch `2025q3`, `keyboards/keychron/common/`), not from documentation, which
doesn't exist. The device is VID `0x3434`, usage page `0xFF60`, usage `0x61`,
and every report is 32 bytes. A reply echoes the request, with byte 2 as the
status (0 means OK) and any results from byte 3 on.

| Bytes | Command | Use |
|---|---|---|
| `A2` | Get supported features | Byte 2, bit 7 set means per-key RGB is available |
| `A8 05` | LED count | Size of the snapshot |
| `A8 06 row m0 m1 m2` | LED index per matrix column | Builds the row map |
| `A8 07` / `A8 08 t` | Get / set per-key type | Force solid (0) while running, restore after |
| `A8 09 start n` | Get per-key colours, n ≤ 9 | Snapshot |
| `A8 0A start n h s v …` | Set per-key colours, n ≤ 9 | Every frame |
| `A8 02` | Save to EEPROM | **Never sent** |

Four facts constrain the design:

1. **Brightness is global.** The solid per-key effect replaces each key's
   brightness with the board's brightness (`per_key_rgb.c`,
   `hsv.v = rgb_matrix_config.hsv.v`). No key can be dark, so an empty segment
   has to be a different colour, not an unlit key.
2. **Colours live in RAM until a save.** glint-keys never saves, so there is no
   EEPROM wear, and unplugging the board always brings back the user's own
   lighting.
3. **The board must be in "Per-key RGB" mode.** The firmware exposes no
   command that names that mode's effect number. Phase 0 finds out whether we
   can determine it. Until then, the user selects the mode once in Keychron
   Launcher.
4. **Only boards built with Keychron's per-key RGB get the `A8` group.** In
   practice that means the Max, HE and newer lines. Older VIA-only Q, V and
   K Pro boards are out of scope for this plan.

## Rows and colours

Bars map to **matrix rows from the top**, in icon order. For a 75% board with
everything shown:

| Matrix row | Keys | Shows | One key ≈ |
|---|---|---|---|
| 0 | Esc, F1–F12, … | CPU | 7% |
| 1 | Number row | Memory | 7% |
| 2 | Q row | GPU | 7% |
| 3 | Home row | NPU | 7% |
| 4 | Shift row | Disk activity | 7% |
| 5 | Bottom row | Network (see [network-kpi.md](../archive/network-kpi.md); built as GLINT-NETWORK-METRIC) | |

- A row with `n` LEDs lights `round(percent / 100 × n)` keys, from the lowest
  matrix column up. The icon rounds its bar lengths the same way, so an idle
  0–3% CPU lights nothing.
- Hiding a measurement moves the bars below it up a row, the same way the icon
  closes the gap.
- Keys in unused rows, and LEDs with no matrix position (side lights,
  underglow), show the track colour.
- **Colours (decision for you):** lit keys are white below 80%, amber at 80%
  and red at 95%. The track is saturated blue.
  - Since no key can be dark, the track needs the dimmest colour an LED can
    show. Saturated blue drives one die, and it has roughly 7% of the perceived
    brightness of white. White lit keys against it give the strongest contrast
    left.
  - The rows drop glint's per-measurement hues. On LEDs, the memory violet and
    disk slate wash out to near-white, and CPU blue would vanish against the
    blue track. Position already tells you which row is which.

## Inside glint-keys

- **Wait loop:** `WaitForMultipleObjects` on tick, stop, glint's process, and
  a device-arrival event that a `CM_Register_Notification` callback for
  `GUID_DEVINTERFACE_HID` sets. No polling and no timer.
- **Attach**, at start and on each arrival:
  1. Enumerate HID interfaces and keep those with VID `0x3434` on usage page
     `0xFF60`.
  2. Require the `A2` feature bit.
  3. Read the LED count, build the row map from `A8 06`, and take a snapshot of
     colours and per-key type.
  4. Set the type to solid.

  Every request flushes the input queue first and reads its reply with a
  500 ms timeout. A board that does not answer is skipped.
- **Frame:** read the feed, compute each LED's HSV, and send only the 9-LED
  chunks that changed since the last frame. A steady machine sends nothing.
- **Detach:** a failed write drops the board, and the next arrival brings it
  back. That covers unplugging and sleep/resume.
- **Clean up**, on stop or glint's exit: write the snapshot back, restore the
  type, then exit.
- **Several keyboards:** each qualifying board is driven, each with its own row
  map.

---

## Crate layout

```
Cargo.toml            [workspace] members = [".", "addons/keys"], same default-members
src/addon.rs          host: discovery, menu items, start/stop/restart
src/feed.rs           Feed layout and seqlock read/write (shared with addons)
addons/keys/          package glint-keys: bin glint-keys, bin keyprobe
addons/<next>/        later addons follow the same shape
```

`cargo build --release` produces every exe side by side in `target\release\`.
Each addon's `windows` features stay in its own `Cargo.toml`. Addons depend on
the `glint` lib only for `feed` and the `AMBER_AT` / `RED_AT` thresholds, so
the thresholds have one source.

## Phases

### Phase 0: keyboard probe (gates Part 2)

`keyprobe` is a probe binary in the addon crate, in the style of `spike` and
`wildcard`.

| Question | How the probe answers it |
|---|---|
| Do your boards have the `A8` group? | `A2` feature bit, LED count, protocol version |
| Is matrix row 0 the physical top row, and do columns run left to right? | `--rows` paints each matrix row its own hue, then `--sweep` lights row 0 one column at a time. You look at the board. |
| Does white on blue read as a bar at your brightness? | `--bars 10,35,60,85,97` paints a still frame |
| Can glint-keys select Per-key mode itself? | Read VIA's current-effect value with Per-key mode on, on each board, to see whether the number can be derived |
| Does the 2.4 GHz receiver carry the raw HID channel? | List what Windows enumerates with the receiver plugged in and the cable out |
| How long does a full frame take? | Time 12 set-colour writes |

Every probe mode restores the snapshot and never sends `A8 02`.

**Gate:** a board without the `A8` group stops Part 2 for that board. If matrix
rows turn out not to be physical rows, the mapping changes to a per-board table
before Phase 2.

### Phase 1: the addon host

- Add `src/feed.rs` and `src/addon.rs`, with config keys and the menu group.
- Add a throwaway `addons/echo` that prints the feed to a console, to prove the
  host before any hardware code exists. It is deleted, or kept as a probe,
  once glint-keys works.

### Phase 2: glint-keys

Build it as described in Part 2.

### Phase 3: spec, README, measurements

- Fold the requirements below into `spec.md`, update the architecture table,
  and add addons to the scope.
- Fix the stale `Requirement prefix: SG` line, which should read `GLINT`.
- README gets an "Addons" section: what they are, where the exe goes, and the
  one-time Launcher step for keyboard lights.
- Record the `glint.exe` size before and after, glint's hidden-idle cost with
  every addon off and with glint-keys on, and glint-keys' private bytes and CPU.

## Spec impact

**New requirements: host**

| ID | Says | Verification |
|---|---|---|
| GLINT-ADDON-DISCOVER | `glint-*.exe` beside glint gets a menu item labelled from `FileDescription`; no addons, no group | manual |
| GLINT-ADDON-COST | All off costs one test per tick: no mapping, no process; glint grows ≤ 4 KB | manual |
| GLINT-ADDON-LIFECYCLE | Start/stop with the setting and with glint; addons exit when glint dies; restart throttled to 60 s | manual |
| GLINT-ADDON-FEED | Feed layout, version and seqlock; bars match `readings()` | **test** |

**New requirements: keyboard**

| ID | Says | Verification |
|---|---|---|
| GLINT-KEYS-ROWS | Bars on matrix rows from the top in icon order; round fill; unused rows and LEDs show the track | manual |
| GLINT-KEYS-COLOR | White, amber at 80, red at 95, on a blue track | manual |
| GLINT-KEYS-RESTORE | Snapshot restored on stop and on glint's exit; never saves to the keyboard | manual, plus the no-save line in GLINT-KEYS-WIRE |
| GLINT-KEYS-HOTPLUG | Plug, unplug, sleep and several boards all recover without restarting glint | manual |
| GLINT-KEYS-WIRE | Exact bytes on the wire | **test** |

The two `test` requirements earn it because a wrong offset or a torn read is
deterministic, cheap to assert, and invisible in code review.

**GLINT-ADDON-FEED, draft acceptance criteria**

- A feed written from a `Metrics` with GPU hidden reads back with
  `bar_count = 4` and `bar_kind = [Cpu, Memory, Npu, Disk]`, matching
  `readings()` for the same input.
- Percents, GHz, the memory byte counts and up to 8 volumes round-trip exactly.
- A read that starts while `seq` is odd retries, and never returns.
- A read where `seq` changed between start and end retries.
- A reader that sees an unknown `version` gets an error, not a block.

**GLINT-KEYS-WIRE, draft acceptance criteria**

- 20 LEDs encode as three set-colour reports with starts 0, 9, 18 and counts
  9, 9, 2. Each begins `A8 0A start count`, carries `h s v` triples, and is
  zero-padded to 32 bytes.
- A frame identical to the last one encodes to no reports. Changing LED 10
  alone encodes to exactly the report starting at 9.
- No report the encoder produces begins with `A8 02` or with VIA's save
  command `09`.

Tests go in `specs/glint/tests/feed.rs` (registered from the root) and
`specs/glint/tests/keys_wire.rs` (registered from `addons/keys/Cargo.toml`).

**Modified requirement:** GLINT-CONFIG-PARSE gains `addon.<name>` keys.
- `{}` gives no addon on.
- `{"addon.keys": true}` turns `keys` on.
- `{"addon.keys": yes}` leaves it off.
- Two addon keys do not cross-match.

The current hand-written reader looks up fixed key names, so it needs a way to
list the `addon.` keys it finds.

**Unchanged:** GLINT-ALERT-COLOR, GLINT-TRAY-BARS and GLINT-IDLE-COST.

## Known limits

- **A killed addon skips its cleanup.** For the keyboard, glint's colours stay
  on the board until it is unplugged. glint only ever terminates after the 1 s
  graceful wait.
- **A keyboard addon restarted after a crash snapshots glint's own colours**,
  so its later restore puts frozen bars back. Unplugging fixes it.
- **Saving in Keychron Launcher while glint-keys runs** would save glint's
  colours as your own.
- **With the window hidden, addons update every 2 s**, at glint's hidden poll
  rate (GLINT-IDLE-COST). An LCD showing numbers may want 1 s; that is a
  decision for when that addon exists.

## Open questions

1. **Keyboard colours:** white, amber and red on blue (recommended), or keep
   the per-measurement hues for the normal state?
2. **Your boards:** which models, and do you use them wired or on the 2.4 GHz
   receiver? Phase 0 needs one plugged in.
