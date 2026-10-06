# glint: tray system monitor (Rust)

Working name: `glint`.

## Goal

A tray icon opens a small borderless window with CPU, memory, GPU, NPU, and disk stats. The window can be pinned on top. Idle cost must be near zero.

## Spec impact

New project. New requirements (to merge into `specs/glint/spec.md` after implementation):

| ID | Requirement | Verification |
|----|-------------|--------------|
| GLINT-TRAY-TOGGLE | Left click on tray icon shows or hides the window | manual |
| GLINT-PIN | Pin button keeps window open and on top until unpinned or the icon is clicked | manual |
| GLINT-LIGHT-DISMISS | Unpinned window hides on focus loss; the icon click right after does not reopen it | manual |
| GLINT-IDLE-COST | Hidden window: poll at most once per 10 s, no rendering | manual |
| GLINT-NPU-OPTIONAL | NPU row is absent when no NPU adapter exists | manual |
| GLINT-ALERT-COLOR | Row turns amber at >= 80% and red at >= 95% | manual |
| GLINT-CLAMP | CPU and GPU percent values are clamped to 100 before display | manual |
| GLINT-CPU-BOOST | CPU row shows current clock speed and the ratio to base clock | manual |
| GLINT-LUID-PARSE | Adapter LUID is parsed correctly from a GPU Engine instance name | test |

`GLINT-LUID-PARSE` is the only requirement that earns a `test`. A parse bug is silent: the row shows 0% forever, and no human catches it in review. The inputs are real strings, and the assertion is a concrete LUID pair.

`GLINT-ALERT-COLOR` is `manual`. A wrong threshold color is visible on screen in one second, so it fails the "silent when wrong" bar.

## Spike 1 findings (2026-10-05, AMD Ryzen AI 9 HX PRO 370, Radeon 890M, Windows 11 26200)

Run them again with `cargo run --release --bin spike`. Add `--watch` for a 30 s live table.

### Risk C: base clock — RESOLVED

| Source | Value |
|--------|-------|
| Registry `~MHz` | 1996 MHz |
| `CallNtPowerInformation` `MaxMhz` | 2000 MHz |

Both report the base clock, not the 5.1 GHz boost ceiling. The code prefers
the registry value and falls back to `MaxMhz`. `% Processor Performance` read
with `PDH_FMT_NOCAP100` gave 1.79x at 58% load, so 3.58 GHz. That matches the
Task Manager Speed readout.

### Risk B: GPU Engine array cost — PASS, no fallback needed

| Measure | Result |
|---------|--------|
| Instances in the array | 491 to 500 |
| `PdhCollectQueryData` | 0.25 ms mean |
| `PdhGetFormattedCounterArrayW` plus parse | 0.18 ms mean |
| Total per tick | 0.43 ms mean, 0.81 ms worst |

Exit criterion was 2 ms. The result is 5 times under it, so the plan keeps a
1 s poll for every row. No worker thread, and no 2 s GPU fallback.
Reused buffers keep a steady tick at zero allocations.

#### The wildcard re-expands on its own — drop the 30 s re-add

The plan said "Re-expand the wildcard every 30 s". A re-add costs one bad
sample, because a fresh rate counter needs two collects. `cargo run --release
--bin wildcard` tests the assumption, and it is wrong.

The probe searches by process id, so the result is decisive. A process that
starts after the add has a process id that no earlier instance name holds.

```
aged counter baseline: 524 instances
started mspaint.exe, pid 51884
aged counter : 557 instances
fresh counter: 557 instances
rows for pid 51884: aged 33, fresh 33
fresh-only: 0   aged-only: 0
```

The counter added before `mspaint` started reports all 33 of its instances.
A counter added afterwards reports the same set. So PDH re-expands a wildcard
on each `PdhCollectQueryData`. The sampler adds each counter once and never
re-adds. The sleep and resume path still re-creates the whole query.

#### A buffer bug that the probe caught

The first version of `Counter::for_each_instance` grew its buffer by a delta:
`items.reserve(needed - capacity + 1)`. That is wrong. `Vec::reserve` asks for
capacity beyond `len()`, and this buffer keeps `len() == 0` always, because PDH
writes the records itself. Once the capacity passed the delta, `reserve`
became a no-op, every later read returned `PDH_MORE_DATA`, and every GPU and
NPU value froze.

The failure was silent: the code discarded the error with `let _ =`. In the
app that is a GPU row stuck at its last value forever.

`Counter::grow_to` now asks for an absolute capacity, and grows by at least
half again, so a retry always makes progress.

### Risk A: NPU counters — RESOLVED, but DXCore is the wrong API

The plan assumed DXCore. DXCore does not see the NPU on this machine:

| API | Adapters reported |
|-----|-------------------|
| DXCore, `D3D12_GRAPHICS` | Radeon 890M, Microsoft Basic Render Driver |
| DXCore, `GENERIC_ML` and `D3D12_CORE_COMPUTE` | none |
| `D3DKMTEnumAdapters2` | Radeon 890M, Basic Render Driver |
| `D3DKMTEnumAdapters3` with `IncludeComputeOnly` | all three, **plus the NPU** |

The XDNA2 NPU driver publishes no Direct3D 12 device, so no DXCore attribute
matches it. `d3dkmthk.h` states the cause for `EnumAdapters2`: "ComputeOnly
adapters are left out of the default enumeration, to avoid breaking
applications."

The answer is `D3DKMTEnumAdapters3` plus `KMTQAITYPE_ADAPTERTYPE`. The
`ComputeOnly` flag, bit 11 of that bitfield, identifies the NPU:

```
luid 0x00000000_0x00016BCB  compute_only=false render=true display=true  sw=false  AMD Radeon(TM) 890M Graphics
luid 0x00000000_0x0001B433  compute_only=true  render=true display=false sw=false  (no name)
luid 0x00000000_0x0001B3D1  compute_only=false render=true display=false sw=true   (Basic Render Driver)
```

Three facts agree that `0x0001B433` is the NPU:

1. WDDM marks it `ComputeOnly` and not a software device.
2. Its `GPU Engine` instances carry only `engtype_Compute`, and only 3
   processes use it: `System`, `svchost`, `WorkloadsSessionHost` (Windows AI).
3. The PnP compute accelerator is `PCI\VEN_1022&DEV_17F0`, the AMD XDNA2 NPU.

`KMTQAITYPE_ADAPTERREGISTRYINFO` returns an empty name for the NPU and for the
Basic Render Driver, so the row label is the constant `NPU`.

Design changes that follow:

- `src/wddm.rs` replaces `src/adapters.rs` as the adapter source.
  `src/adapters.rs` stays for the DXCore comparison in the spike.
- Keep `GLINT-NPU-OPTIONAL`: `D3DKMTEnumAdapters3` needs WDDM 2.9, and the code
  falls back to `EnumAdapters2`, which hides the NPU.

**Exit criterion met (2026-10-05).** An NPU workload ran while both this app
and `Get-Counter` sampled the same counters for 14 seconds. Below 100 the two
agree within about 2 points, against the 5 points the criterion allowed:

| Independent read | glint |
|------------------|------------|
| 107.3 / 93.1 / 108.8 / 92.2 | 100 / 94 / 100 / 94 |
| 107.9 / 93.6 / 92.5 / 107.0 | 100 / 94 / 94 / 100 |
| 92.3 / 108.6 / 93.4 / 92.4 | 94 / 100 / 94 / 94 |
| 30.8 / 0.0 | 16 / 0 |

The readings above 100 are clamped, which is GLINT-CLAMP working on real data.
The tail differs because the burst ended between the two samplers.


## Non-goals

- Per-process lists (Task Manager does that).
- Temperatures and fan speeds.
- Taskbar embed or AppBar docking (possible later, see "Later").
- History beyond 60 seconds.

## Stack

- Rust, stable toolchain, MSVC target.
- `windows` crate (Microsoft) for Win32, PDH, DXCore, DWM, GDI.
- **GDI with a double buffer** draws the window. No Direct2D, and no Direct3D.
  Reason: a Direct2D render target creates a D3D device. That costs RAM, and it holds the GPU awake. Four rows and four sparklines need no GPU. GDI draws them in under a millisecond.
- No UI framework. This keeps RAM near 10 MB.
- Release profile: `opt-level = "s"`, `lto = true`, `panic = "abort"`, `strip = true`.

## Data sources

| Row | Source | Notes |
|-----|--------|-------|
| CPU load | PDH `\Processor Information(_Total)\% Processor Utility` | Exceeds 100 above base clock. Clamp to 100 (GLINT-CLAMP). |
| CPU speed | PDH `\Processor Information(_Total)\% Processor Performance` | Times base clock. Gives GHz and the boost ratio (GLINT-CPU-BOOST). |
| Memory | `GlobalMemoryStatusEx` | Show used / total and percent. |
| GPU | PDH `\GPU Engine(*)\Utilization Percentage` | Group by adapter LUID. Sum per engine type, take the max across engine types, then clamp to 100. |
| NPU | Same `GPU Engine` counters, filtered to the NPU adapter LUID | Adapter found by DXCore enumeration. See Spike 1. |
| Disk activity | PDH `100 - \PhysicalDisk(_Total)\% Idle Time` | This is the "Active time" value that Task Manager shows. Do not use `% Disk Time`: it is a legacy counter, it exceeds 100% on multi-queue devices, and it misleads. |
| Disk space | `GetDiskFreeSpaceExW` per fixed drive | Poll every 30 s. Collapsed by default. |

PDH rules:

- Create each query once. Add wildcard counters with `PdhAddEnglishCounterW`, so names work on any locale.
- Call `PdhCollectQueryData` on each tick, then `PdhGetFormattedCounterArray`.
- The `GPU Engine(*)` instance set changes as processes start and stop. Re-expand the wildcard every 30 s.
- Clamp every percent value to the range 0 to 100 at the display layer (GLINT-CLAMP).

## Spike 1: NPU counters and PDH cost (do first)

Three risks live here. Answer all three before you write any UI code.

### Risk A: no simple NPU counter exists

Findings so far come from Microsoft Q&A threads, not official docs.

1. Enumerate adapters with DXCore. Filter by `DXCORE_ADAPTER_ATTRIBUTE_D3D12_GENERIC_ML` (or the compute-only attribute). Print the LUID and the description.
2. List `GPU Engine(*)` instances with `typeperf -qx "GPU Engine"` or PDH. Find instances whose `luid_0x..._0x...` matches the NPU LUID.
3. Run a known NPU load. Use Windows Studio Effects, or an ONNX Runtime DirectML/QNN sample.
4. Confirm the counter moves, and confirm it matches Task Manager.

Exit criterion: one number that tracks the Task Manager NPU graph within about 5 points.

Fallback: ship without the NPU row, and keep `GLINT-NPU-OPTIONAL`.

### Risk B: the GPU Engine array is the heaviest operation

`GPU Engine(*)` expands to one instance per process per engine. A busy machine gives hundreds. `PdhGetFormattedCounterArray` allocates for all of them, once per second.

1. Count the instances on an idle machine, and count them under load.
2. Time `PdhCollectQueryData` plus `PdhGetFormattedCounterArray` over 100 calls.

Exit criterion: under 2 ms per call.

Fallback: poll the GPU and NPU every 2 seconds. Poll the CPU, memory, and disk every second.

### Risk C: the base clock source

The base clock source is inconsistent across hardware.

- The registry value `~MHz` under `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0` is the usual choice.
- `CallNtPowerInformation` with `ProcessorInformation` reports `MaxMhz`. Some systems report the boost ceiling there instead of the base clock.

Confirm which source gives a GHz value that matches the Task Manager Speed readout.

## Window behavior

- Tray icon via `Shell_NotifyIconW`. Handle `TaskbarCreated` to re-add the icon after an Explorer restart.
- Window: `WS_POPUP`, `WS_EX_TOOLWINDOW` (no taskbar button), rounded corners and Mica/acrylic via DWM attributes.
- Position: above the icon rect from `Shell_NotifyIconGetRect`. Clamp to the monitor work area. Handle multi-monitor and DPI.
- Toggle: left click shows or hides.
- Focus: call `SetForegroundWindow` after you show the window. A `WS_EX_TOOLWINDOW` popup does not take focus on its own, and light dismiss needs focus.
- Light dismiss: on `WM_ACTIVATE` deactivate, hide the window when it is not pinned. Record the hide time. Ignore an icon click within 200 ms of it, to prevent the reopen flicker.
- Pin: button in the window corner. Pinned adds `WS_EX_TOPMOST` and skips light dismiss.
- Right click on icon: menu with Pin, Start with Windows, Exit.
- Persist pin state and window position in `%APPDATA%\glint\config.json`.
- Single instance via a named mutex.
- Re-create the PDH queries after a sleep and resume cycle. Handle `WM_POWERBROADCAST`.

## Poll and render policy

| State | Poll | Render |
|-------|------|--------|
| Window visible | 1 s | Redraw on each tick |
| Window hidden | 10 s (tray tooltip only) | None |

- Use a timer (`SetTimer` or a waitable timer) on the UI thread. No busy loop. Add a worker thread only if Spike 1 shows that PDH blocks.
- Tray tooltip text: `CPU 12%  MEM 48%  GPU 3%  NPU 0%`.

## Layout

One row per resource: label, percent, and a 60-sample sparkline.

- The CPU row carries an extra readout: `CPU 98%  4.21 GHz (1.4x)`.
- Disk is a collapsed section under the main rows.
- Colors: one hue per resource. Amber at 80%, red at 95%.
- The sparkline ceiling is a fixed 100%. Values are clamped, so the shape stays comparable across time.

## Milestones

1. Spike 1: risks A, B, and C. Output: notes in this file.
2. PDH sampler for CPU load, CPU speed, memory, GPU, and disk. Console output only.
3. Tray icon and toggle window with static text.
4. GDI rows and sparklines.
5. Pin, light dismiss, flicker guard, config persistence.
6. Start with Windows (Run registry key), polish, release build.
7. Measure: RAM and CPU while visible and hidden. Targets: under 15 MB, under 0.5% CPU visible, under 0.1% hidden.

## Tests

`GLINT-LUID-PARSE` only. Assert the parse of real instance names, for example:

```
luid_0x00000000_0x0000CAFE_phys_0_eng_1_engtype_3D
```

Expected: high `0x00000000`, low `0x0000CAFE`, phys `0`, eng `1`, engtype `3D`. Add a malformed input case, and assert that it returns an error rather than a wrong LUID.

Location: decide at milestone 2. Use `specs/glint/tests/`, or a cargo `#[test]` if the home repo layout prefers that.

All other requirements get a manual procedure in the spec, under **Verification (manual):**. They get no test file.

## Later (not in scope now)

- AppBar strip docked above the taskbar. Reuse the same render code.
- Settings for poll rate and thresholds.

## Open questions

- Does your NPU expose engine counters? Risk A answers this.
- Home repo for the spec: this new `glint` repo is assumed.
