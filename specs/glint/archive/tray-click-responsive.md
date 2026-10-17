# Tray clicks that never land: move sampling off the UI thread

**Outcome (2026-10-16, built):** shipped as GLINT-TRAY-RESPONSIVE. Worst UI stall
under the same load fell from 1933 ms to 114 ms. `Feed` carries `Control`s one
way and `Sample`s the other. Hidden-idle CPU and private bytes were not
re-measured with the worker in place.

Written for: Cyrus, reviewing before implementation.

## The bug

Sometimes a left click on the tray icon does nothing. It happens more when the
machine is busy.

## Cause, measured on 2026-10-16

glint is not short of CPU. The problem is that the thread that handles clicks
is also the thread that samples, and one sampling call waits on the system for
a second or more when the machine is busy.

`Sampler::tick` runs inside `WM_TIMER` on the UI thread (`window.rs:868`). Every
10th tick it calls `GetIfTable2` to rescan network adapters
(`sampler.rs:366`). Every tick it calls `GetIfEntry2` once per live adapter
(`sampler.rs:370`). Both calls cost a few milliseconds at idle and get far
slower under load:

| Call | Idle | Loaded: 48 spinning threads on 24 logical CPUs |
|---|---|---|
| `GetIfTable2` | 3.6 ms | 979 ms median, 1.12 s worst (2.6 s with the load inside the same process) |
| `GetIfEntry2`, all 6 hardware adapters | 0.6 ms | 144 ms median |
| Each PDH collect, one counter per query | 1 to 3 ms | under 1 ms median, 40 ms worst |
| `NIM_MODIFY` to Explorer | 2 ms | 0.9 ms median, 3 ms worst |
| Whole `Sampler::tick` | 8.6 ms median | 41 ms median; **1.08 s and 1.14 s on the rescan ticks** |

The load ran in a separate process for the `GetIfTable2`, `GetIfEntry2` and
whole-tick rows, and inside the measuring process for the PDH and `NIM_MODIFY`
rows.

So for a second or more out of every 10 to 20, the UI thread is stuck inside
`GetIfTable2`. A click made during that time waits in the message queue. If you
click again because nothing happened, both clicks are processed together when
the call returns: the window shows, then hides before it is ever painted.
Nothing pops. `show()` also runs a full tick before the window can paint
(`window.rs:338`), so even a single click gets slower under load.

This holds when the Network row is switched off, because `read_network` runs on
every tick whether or not the row is shown.

Two parts of the spec rested on idle averages and no longer hold. The
**Measured cost of one tick** section says 0.43 ms mean, "so there is no worker
thread". GLINT-NETWORK-METRIC says `GetIfTable2` "lists the adapters in about
1 ms". The Network row arrived in 0.3.0. If the dead clicks began around then,
that fits.

Scratch harnesses that took these numbers (not for the repo):
`C:\Users\josha\AppData\Local\Temp\claude\c--code-glint\2e00274b-4343-4a1f-b1a5-245d31405ba8\scratchpad\ticktime\`.

## Fix

The sampler moves to its own thread. The UI thread never makes a sampling call,
so the only things it waits on are clicks, paints, and the tray icon update. The
tray icon update stayed under 3 ms under the same load.

```
sampler thread                         UI thread
  Sampler::new()  -- caps ----------->  App::start (waits for this once)
  loop:
    wait for Control or the period
    tick()        -- Sample ---------->  WM_SAMPLE: history.push, update_tray,
    PostMessage(WM_SAMPLE)                           paint if visible
                  <-- Control ---------  show/hide: Period(1 s / 2 s)
                                         show: Now
                                         resume from sleep: Rebuild
```

- The sampler thread builds the `Sampler`, so PDH handles never cross threads and
  nothing needs an `unsafe impl Send`. It sends back `Caps` (`gpu`, `npu`,
  `temp`, `network`) or the `PdhError`. `App::start` waits for this, so the
  "glint could not start" dialog behaves as it does today.
- Each `Sample` carries `Metrics` plus the current `Caps`, because `network`
  can change at a rescan. The UI thread reads every queued sample on
  `WM_SAMPLE`, so history keeps every reading even if the UI was slow.
- Plain `std::sync::mpsc` channels. The worker waits with `recv_timeout`, so
  there is no Win32 event and no new `windows` feature.
- `show()` paints the last reading straight away, which is at most one hidden
  poll (2 s) old, and sends `Now`. The fresh reading arrives one tick later.
  Today the window waits for a tick before it can paint at all.
- Shutdown: dropping `App` drops the sender. The worker sees the channel close
  and returns. There is no join, because a tick in progress can take seconds,
  and exit should not wait for it. A `PostMessage` to the destroyed window just
  fails.
- `Sampler`'s own API (`new`, `tick`, `rebuild`, `has_*`) does not change, so
  the `sample` probe keeps working.

## Changes

| File | Change |
|---|---|
| `src/sampler.rs` | Add `Caps`, `Sample`, `Control`, and `pub fn spawn(notify) -> Result<(Feed, Caps), PdhError>`. The `notify` closure keeps `HWND` out of this module. Rewrite the module doc at `:1-4`. |
| `src/window.rs` | `App.sampler` becomes `feed: Feed` and `caps: Caps`. Build the window before calling `spawn`, because `notify` needs the `HWND`. New `WM_SAMPLE = WM_APP + 2`. Delete `TIMER_POLL` and `poll()`, and put a `WM_SAMPLE` handler in its place. `set_poll_rate` sends `Period`. `show()` sends `Now`. `WM_POWERBROADCAST` sends `Rebuild`. Every `self.sampler.has_*()` reads `self.caps`. |
| `src/bin/sample.rs` | Report the 99th percentile and worst tick, not only the mean, since the mean is what hid this. Add `--burn`, which keeps 2 spinning threads per logical CPU busy during the run. It is the load for the manual test below. |
| `specs/glint/spec.md` | See Spec impact. |

There is no new dependency and no new `windows` feature. The release exe gains
one thread.

## Spec impact

**New: GLINT-TRAY-RESPONSIVE** (manual)

> A click on the tray icon shall show or hide the window at once, however busy
> the machine is. Sampling runs on its own thread, so a slow system call
> delays the next reading, never the click.
>
> **Verification (manual):**
> 1. Start glint with the window hidden.
> 2. Run `cargo run --release --bin sample -- 60 --burn`. The worst tick it
>    reports is over 500 ms, which shows the load reproduces the stall.
> 3. While it runs, left click the tray icon 20 times, about once a second.
>    Every click toggles the window, with no visible delay.
> 4. Right click the tray icon. The menu opens at once.

**Modified:**

- **Measured cost of one tick**: keep the history, add the 2026-10-16 table, and
  replace "so there is no worker thread" with the reason there is one.
- **GLINT-NETWORK-METRIC**: "about 1 ms" becomes "about 1 to 4 ms at idle, and
  1 to 2.6 s on a saturated machine (see GLINT-TRAY-RESPONSIVE)".
- **GLINT-IDLE-COST**: no change to the rule. Re-measure hidden CPU and private
  bytes with the worker in place, and add a row to the table.
- **Architecture table**: `src/sampler.rs` becomes "Metric sampling, on its own
  thread".

GLINT-RESUME is unchanged. The rebuild just runs on the sampler thread now.

## Not doing

- **Treating a double click as one click** (Qt ignores the `NIN_SELECT` that
  follows `WM_LBUTTONDBLCLK`). A quick hide-then-show would become a click that
  does nothing, which is this bug again. Once clicks land at once, two clicks
  correctly mean show, then hide.
- **Raising the UI thread's priority.** No measurement calls for it. Revisit
  only if step 3 above shows a lag with the worker in place.
- **Caching or event-driving the adapter rescan** (`NotifyIpInterfaceChange`).
  Once it is off the UI thread, a slow rescan only delays the icon's next
  update by a second, now and then, under heavy load.
- **`PostMessage(WM_NULL)` after `TrackPopupMenu`** (the old KB135788 rule) and
  checking the `NIM_SETVERSION` result. Both are real gaps, but nothing ties
  them to this symptom. They would be a separate change.
