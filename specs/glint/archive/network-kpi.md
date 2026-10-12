# Network: a sixth measurement

Written for: Cyrus, reviewing before implementation.

**Outcome (2026-10-12, built):** shipped as GLINT-NETWORK-METRIC, with two
departures from this plan. The PDH `Network Interface` counters cost 2.3 to
2.8 ms per collect (`netcheck`), more than the rest of the tick, so the
sampler reads `GetIfTable2` and `GetIfEntry2` instead (+0.45 ms per tick). That
also settled Step 0 question 3, since the adapter list is rescanned every 10
ticks. Direction followed the recommendation, `max(received, sent)` against
each direction's own link speed. Task Manager's figure was not compared. Tray
strips now use rounded edges so six strips fill all 16 rows.

Network utilisation joins CPU, memory, GPU, NPU and disk activity as a
measurement: a row in the window, a strip in the tray icon, a menu toggle, and
a bar for any addon. It is core glint, not an addon.

## Measure first (before any code)

Nothing below has been run on this machine yet, because the shared-machine lock
was held when this was planned. Step 0 settles three things the design depends
on:

1. Which counters exist here: `\Network Interface(*)\Bytes Received/sec`,
   `Bytes Sent/sec`, `Bytes Total/sec`, and `Current Bandwidth` (bits per second).
2. What Task Manager's Ethernet or Wi-Fi utilisation figure is computed from.
   Compare it with `Bytes Total/sec × 8 / Current Bandwidth` and with
   `max(received, sent) × 8 / Current Bandwidth` while a download runs.
3. Whether the `Network Interface(*)` wildcard picks up an adapter that appears
   after the query was built (Wi-Fi reconnect, VPN). GLINT-RESUME and the
   wildcard probe show `GPU Engine(*)` does; this one is unproven.

A new probe binary, `netcheck`, answers them, in the style of `cpucheck`. The
results go into the spec as a table, as the CPU metric's did.

## Design

**The value.** Percent of the link, for the busiest adapter, clamped to 0 to 100
(GLINT-CLAMP). "Busiest adapter" mirrors GLINT-GPU-BUSIEST: summing adapters
would count a laptop's Wi-Fi and Ethernet at once, and VPN and virtual
adapters double-count traffic that also crosses the physical one.

**Direction (decision for you).** On Ethernet, which is full duplex, a link
saturated in one direction reads as 50% of `Bytes Total/sec`. I recommend
`max(received, sent)` over `Current Bandwidth`, so a full-speed download reads
100%. Wi-Fi is half duplex, where the total is the honest figure, but one
formula that is right for the common case beats two. Step 0 says what Task
Manager does, and I'd follow it unless it is clearly wrong.

**Which adapters count.** Skip loopback and adapters whose `Current Bandwidth`
is 0. Hyper-V, WSL and other virtual adapters report a nominal 10 Gbps, so
their percentages stay tiny and the busiest-adapter rule hides them.

**Where it sits.** After disk activity in the window and the tray icon, before
disk space:

```
CPU, Memory, GPU, NPU, Disk activity, Network, Disk space
```

**Absent like the NPU.** A machine with no up network adapter shows no Network
row, and the menu item is not offered (GLINT-NPU-OPTIONAL's rule, same
reasoning: never offer a toggle that does nothing).

## An honest caution

Percent of link speed sits near 0% almost all the time on a fast link. A
1 Gbps connection pulling a 100 Mbps video stream reads 10%. The bar will be
empty on most days and amber only during a large transfer. That matches what
was asked, and it makes the row accurate but quiet. If it turns out too quiet to
be useful, the alternative is throughput against a fixed or self-calibrating
ceiling, which is a different number and its own decision. I would not build
that speculatively.

## What changes

| Area | Change |
|---|---|
| `sampler.rs` | New counters, `Metrics.network: Option<f32>`, `Histories.network` |
| `config.rs` | `network: bool` in `Show`, key `"network"`, default visible |
| `render.rs` | `NETWORK_HUE`, `Shown.network`, one more row in the layout and paint |
| `trayicon.rs` | One more entry in `bars()`, so the icon and hover text both gain it |
| `window.rs` | Menu id and item, toggle in `on_command` |
| `specs/glint/tests/config_parse.rs` | New criteria below |

## Risks

- **The tray icon gets crowded.** The icon is 16 pixels tall. Six strips are
  2.67 pixels each, where five were 3.2, and the track already stops one row
  short of each pitch. Strips will have uneven heights of 2 and 3 pixels. Look
  at it before deciding. If it reads badly, the fallback is drawing network
  only in the window and not in the icon, which breaks "the icon shows the
  same measurements" and needs your call.
- **Everyone gets a sixth strip on upgrade**, because a missing config key
  means visible. That is the rule GLINT-CONFIG-PARSE already states, so I would
  keep it.
- **Cost.** One more wildcard counter. The spec's tick budget is 0.43 ms mean;
  measure the new figure with the same method, and record it.

## Spec impact

**New requirement**

| ID | Says | Verification |
|---|---|---|
| GLINT-NETWORK-METRIC | Network percent is the busiest up adapter's utilisation of its link, per the formula Step 0 selects, clamped; absent with no adapter | manual |

**Modified requirements**

- **GLINT-KPI-TOGGLE:** seven items instead of six; **Network** appears only
  when an adapter exists.
- **GLINT-TRAY-BARS:** six strips, a `NET` line in the hover text.
- **GLINT-FIT:** a Network row adds one row of height; the size table is
  re-measured.
- **GLINT-ALERT-COLOR:** applies to the Network row with no change to the
  thresholds.
- **GLINT-CONFIG-PARSE (test):** gains the criteria below.
- **GLINT-IDLE-COST:** re-measured with the new counter.

**GLINT-CONFIG-PARSE, new acceptance criteria**

- `{}` gives `network = true`.
- `{"network": false}` hides the Network row and leaves the other measurements
  as they were.
- `{"network": }`, `{"network": yes}` and `{"network":` each leave it visible.
- The eight keys do not cross-match each other.

## Effect on the other plans

- **Addon feed** ([keyboard-rows.md](keyboard-rows.md)): `bar_kind` and
  `bar_percent` grow from 5 to 6 entries, and the kinds gain `Network`. The
  feed layout is not built yet, so this costs nothing now and would cost a
  version bump later. Another reason to land this first.
- **Keyboard rows:** the bottom row, which was track only, becomes Network on
  a board with six matrix rows.
- **Click to open Task Manager:** Network maps to the Network column, so the
  Task Manager plan gets a sixth target.

## Order of work

1. `netcheck` probe and the Step 0 measurements.
2. Sampler, config and tests.
3. Window row, menu item and tray strip.
4. Spec, README and the re-measured cost table.
