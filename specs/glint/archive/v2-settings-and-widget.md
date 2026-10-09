# glint v2: menu settings and a tight fit. v3: a real Widgets Board provider.

Written for: Cyrus, reviewing before implementation.

---

## OUTCOME (archived 2026-10-08)

**Phase A shipped as written.** GLINT-KPI-TOGGLE, GLINT-FIT and
GLINT-DISK-LINE are in `spec.md`; GLINT-CONFIG-PARSE gained six booleans and
six tests.

**Phase B was abandoned, and the reason invalidates its premise.** The goal
was a reading rendered on the taskbar, like the weather entry point. That
entry point is the Widgets button itself, owned by Microsoft, and a
third-party provider cannot write to it — the provider manifest schema
declares only board sizes, an icon and picker screenshots, with no taskbar
surface at all. A Phase B widget would have been a card inside the Win+W
board, behind MSIX packaging, a COM server and the Windows App SDK.

It was replaced by **GLINT-TRAY-BARS**: the tray icon itself became a live bar
chart. Same surface the user wanted, none of the packaging, and it reuses the
runtime icon drawing glint already had.

The W1 spike result below stands and is worth keeping — the Rust side of a
widget provider does work, should the board ever be wanted as a second
surface. W2 through W5 were never run.

The plan below is the version reviewed before implementation, left unedited.

---

Plan location follows the spec doctrine (`specs/{feature}/plans/`) rather than
`.claude/plans/`, because this repo commits its plans and archives them beside
the spec. The previous plan sits in `specs/glint/archive/tray-monitor.md`.

## The window stays. The widget is additive.

This is a yes/and, and nothing below should be read any other way.

The tray icon keeps opening the window it opens today, with the Phase A
changes applied to it. The board widget is a **second**, independent surface
for the same numbers. Either can be used without the other:

| | Tray window | Board widget |
|---|---|---|
| Opened by | Left click the tray icon | Pinned in the Widgets Board |
| Shows | Every row, sparklines, GHz and GiB detail | Letters, bars, and numbers |
| Runs | `glint.exe`, always | `glint-widget.exe`, only while the board is open |
| Needs the other? | No | No |

What the last draft **cut** was the in-window compact mode — letters and bars
drawn inside glint's own popup as a view toggle. That is gone because it was a
fake widget. The popup itself is untouched and is Phase A's entire subject.

## What this plan covers

Two phases, because they have nothing in common but the word "widget".

**Phase A (v2)** is the window work: menu settings, a window that fits its
content, and a one-line disk space readout. No new dependencies, no packaging,
ships on its own.

**Phase B (v3)** is a genuine Windows 11 Widgets Board provider: an MSIX
package, an out-of-process COM server, and an Adaptive Card. It is gated on a
spike, because this repo's method is to disprove the risky assumption before
building on it — the way Spike 1 disproved DXCore for the NPU.

The in-window "widget view" from the first draft is **cut**. You were right
that scaling down the popup is not a widget; it is a smaller popup.

---

## Measured on this machine before planning (2026-10-08, Windows 11 26200.8655)

| Check | Result |
|-------|--------|
| `HKCU\...\Explorer\Advanced\TaskbarDa` | `0x1` — widgets are on now |
| `Microsoft.WidgetsPlatformRuntime` | 1.6.19.0, installed |
| `MicrosoftWindows.Client.WebExperience` | 526.21100.40.0, installed |
| `Microsoft.WindowsAppRuntime.1.8` | 8000.770.947.0 x64, installed |
| Windows SDK | 10.0.26100.0 — `makeappx.exe` and `signtool.exe` present |
| Rust | 1.92.0, MSVC |

Every prerequisite for Phase B is already on the box. Nothing needs installing
to run the spike.

---

# Phase A — v2

## A1. Settings in the right click menu

No gear button. The existing menu grows, and it already opens from both the
tray icon (`WM_TRAY`) and the window itself (`WM_CONTEXTMENU`).

```
  CPU                        v
  Memory                     v
  GPU                        v     (absent when no adapter)
  NPU                        v     (absent when no NPU)
  Disk activity              v
  Disk space                 v
  ---------------------------
  Add widget to the board...       (greyed when widgets are off)
  ---------------------------
  Hide
  ---------------------------
  Start with Windows
  Reset position
  Exit
```

- Each measurement is `MF_CHECKED` while it is on. Menu ids 110 to 116, after
  the existing 102 to 104.
- GPU and NPU appear only when the hardware does, so GLINT-NPU-OPTIONAL stays
  honest: a machine with no NPU never offers a toggle that does nothing.
- **Hide** is new, and is simply the minimize button as a menu item.
- **Add widget to the board...** is where your "grey it out when widgets are
  off" instinct lands correctly. It opens the Widgets Board, and it is
  `MF_GRAYED` when `TaskbarDa` is `0` or the `AllowNewsAndInterests` policy
  says no. In Phase A the item is absent entirely; it arrives with Phase B.

Toggling writes the config, saves, and calls `refit()` when visible. While
hidden it only writes: the next `show()` measures from scratch anyway.

**Hiding everything is allowed.** The panel then draws one dim line, `Right
click to choose measurements`, and sizes to it. Blocking the last toggle needs
a greyed item that explains itself; an empty panel that says what to do is
simpler and recoverable.

## A2. The window fits its content

### Height

`Layout::new` currently hardcodes `detail_rows = 2` and
`plain_rows = 1 + has_npu + 1`. It takes a resolved visibility set instead:

```
pad
+ HEADER
+ ROW + DETAIL   if cpu            (the GHz line)
+ ROW + DETAIL   if memory         (the GiB line)
+ ROW            if gpu
+ ROW            if npu
+ ROW            if disk activity
+ ROW            if disk space     (the new single line)
+ pad
```

The `SEPARATOR` band and the separate "Disk space" title row both go, and the
per-volume rows collapse into one line. With everything on and one drive the
panel loses 30 logical pixels; with CPU and memory alone it is under half its
current height.

### Width

Natural width stays 300 logical pixels. The disk space line is the one piece
that can need more, so the panel widens to fit it and never narrows below 300.

Measuring needs a DC and the right font, which `Layout::new` does not have.
Two small additions to `Renderer`:

- `ensure_fonts(dpi)` — split out of `prepare()`, so fonts can exist before the
  back buffer does.
- `disk_line_width(dpi, volumes) -> i32` — `GetTextExtentPoint32W` over the
  label and each `C: 95%` token, plus the gaps.

`App` owns both renderer and layout, so the calls are sequential and the borrow
checker is content:

```rust
let disk_width = self.renderer.disk_line_width(self.dpi, &self.metrics.volumes);
self.layout = Layout::new(self.dpi, shown, volumes, disk_width);
```

### A stable visibility set

`Shown` resolves the user's choice against the hardware:

```rust
shown.gpu        = config.show.gpu        && sampler.has_gpu();
shown.npu        = config.show.npu        && sampler.has_npu();
shown.disk_space = config.show.disk_space && !volumes.is_empty();
```

`sampler.has_gpu()` is new and mirrors `has_npu()`. The obvious
`metrics.gpu.is_some()` is wrong here: `gpu` is `None` on the unprimed first
tick, so the window would open one row short and grow a second later. A LUID
found at startup does not come and go, so it is the stable predicate.

## A3. The disk space line

Before: a separator rule, a "Disk space" title row, then one row per drive with
a letter, a percentage and a progress bar, all at `font_detail` (11px).

After: one line, no rule, no bars, everything at `font_label` (12px).

```
Disk space   C: 95%   Z: 41%
```

- `Disk space` keeps `TEXT_DIM` and `font_label` — already 12px, unchanged.
- `C:` in `DISK_HUE`, the percentage in the GLINT-ALERT-COLOR colour, both at
  `font_label`. So only the drive tokens move, 11px to 12px, as you asked.
- Tokens lay out left to right from measured widths, not a fixed column.

## Phase A spec impact

### New

| ID | Requirement | Verification |
|----|-------------|--------------|
| GLINT-KPI-TOGGLE | The right click menu shows or hides each measurement, and the choice survives a restart | manual |
| GLINT-FIT | The window is exactly as tall and wide as the measurements it draws | manual |
| GLINT-DISK-LINE | Disk space is one line of percentages at the body font size | manual |

None earns a `test`. Each is wrong in a way a human sees in one second: a
missing row, a gap at the bottom, a font a size too small. That fails the
"silent when wrong" bar, so the procedure goes in the spec and no file is
created for it.

### Modified

| ID | Change | Verification |
|----|--------|--------------|
| GLINT-CONFIG-PARSE | Six new visibility booleans | `test` (extend) |
| GLINT-NPU-OPTIONAL | NPU row absent with no NPU **or** when hidden in the menu | manual |
| GLINT-ALERT-COLOR | Disk space has coloured text but no bar | manual |
| GLINT-PLACEMENT | Refit also fires on a menu toggle, not only on a drive change | manual |

`GLINT-CONFIG-PARSE` keeps its `test`. A boolean that silently fails to parse
resets a choice with no error anywhere, which is the silent failure the
doctrine reserves `test` for.

### Removed

None.

## Phase A implementation order

1. **`src/config.rs`** — `Show` (six booleans, all default true). `read_bool`.
   `save` writes every key. `Config` loses derived `Default`.
2. **`specs/glint/tests/config_parse.rs`** — extend first, red before green.
   The new cases fail to compile until step 1 lands: a missing field, not a
   `not implemented` stub.
3. **`src/sampler.rs`** — `has_gpu()`.
4. **`src/render.rs`** — `Shown`; `Layout::new` takes it plus the measured disk
   width; drop the separator band and title row; add the empty state.
5. **`src/render.rs`** — `ensure_fonts`, `measure`, `disk_line_width`.
6. **`src/render.rs`** — honour `Shown` in `paint`; replace
   `draw_disk_section` with the one-line version.
7. **`src/window.rs`** — menu items, `on_command`, refit on toggle.
8. **Docs** — merge requirements into `spec.md`, refresh the README menu table,
   retake `docs/screenshot.png`.

---

# Phase B — v3, the Widgets Board provider

## About the hover text

You liked the per-cell hover text, and I have to be straight that it does not
survive the move to the board. Adaptive Cards have no hover or tooltip concept;
the only candidate is `Image.altText`, which is an accessibility string and is
probably read by a screen reader rather than shown on hover. It is on the spike
list, but plan for it not working.

What replaces it is better anyway: the card has room, so the numbers are simply
*on* it. `$host.widgetSize` renders one template three ways.

| Size | Shows |
|------|-------|
| small | letters and bars only — the glanceable version you drew |
| medium | letters, bars, and the percentage under each |
| large | medium, plus used / total GiB per drive |

## What the platform requires

Confirmed from Microsoft's Win32 C++/WinRT walkthrough:

1. **Packaged only.** "In the current release, only packaged apps can be
   registered as widget providers." MSIX, signed. The loose 195 KB exe is no
   longer the whole story.
2. **Out-of-process COM server.** A CLSID, an `IClassFactory`,
   `CoRegisterClassObject(CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)`, then
   `CoWaitForMultipleObjects` until the host releases it.
3. **Two manifest extensions.** `com:Extension Category="windows.comServer"`
   with a `com:ExeServer`, and `uap3:AppExtension
   Name="com.microsoft.windows.widgets"` carrying the `WidgetProvider`
   definitions, icons, screenshots and declared sizes.
4. **Windows App SDK types.** `Microsoft.Windows.Widgets.Providers`, which is
   **not** in the `windows` crate. It ships as a `.winmd` in the Windows App
   SDK package.
5. **No bootstrapper.** Because the app is MSIX-packaged and
   framework-dependent, the framework dependency is declared in the manifest
   and `MddBootstrapInitialize` is not called. That is the single biggest
   simplification for a Rust implementation — the bootstrapper is the part
   that is genuinely hostile to non-.NET toolchains.
6. **VCLibs** is a second framework dependency. Already installed here.
7. **Content is Adaptive Card JSON** — a template plus a data payload, passed
   through `WidgetUpdateRequestOptions` to
   `WidgetManager::GetDefault().UpdateWidget(...)`.

### Update cadence

There is **no documented throttle**. The docs say only that `Activate` and
`Deactivate` "define a window in which the widget host is most interested in
showing the most up-to-date content", that providers may update at any time but
should balance freshness against battery, and that "the time window between
Activate and Deactivate may be small".

So the design is: sample and push on a 1 s timer between `Activate` and
`Deactivate`, and push nothing at all outside that window. The board keeps
showing the last card it got, which for a system monitor is the right
behaviour — a stale number nobody is looking at costs nothing. Spike W4
measures what the host actually honours.

## Architecture: a second exe in one package

```
glint/                      workspace root
  crates/glint/             the library + the tray exe  (unchanged, no new deps)
  crates/glint-widget/      the COM server exe          (WinAppSDK bindings)
  packaging/
    Package.appxmanifest
    ProviderAssets/         icons and picker screenshots
    build.ps1               makeappx + signtool
```

Two executables, one MSIX. `com:ExeServer Executable=` takes a package-relative
path, so the Widgets host launches `glint-widget.exe` and never touches the
tray app.

This matters. It keeps the tray app exactly what it is today — 195 KB, one
dependency, no runtime — and quarantines the Windows App SDK inside a process
that only runs while the board is open.

**The provider samples for itself.** It links `glint::sampler` and runs its own
PDH query while activated. No IPC, no shared memory, and the widget works when
the tray app is not running. A tick is 0.43 ms, and only happens while someone
is looking at the board.

### Rust bindings — W1, RESOLVED 2026-10-08

**It works, and no toolchain change is needed.** Full evidence below under
*Spike 2 findings*. The recipe:

| Piece | Value |
|-------|-------|
| Metadata | `Microsoft.Windows.Widgets.winmd`, 36,384 bytes, taken straight from the installed `WindowsAppRuntime.1.8` package |
| Generator | `windows-bindgen` **=0.66.0** — the version that pairs with the `windows 0.62.2` already in `Cargo.toml` |
| Filter | `Microsoft.Windows.Widgets.Providers` — **not** the parent namespace (see below) |
| Flag | `--implement`, which emits the `_Impl` traits |
| Output | 3,977 lines, depending only on `windows-core`, `windows-collections`, `windows-future` |

```rust
windows_bindgen::bindgen([
    "--in", "Microsoft.Windows.Widgets.winmd", "default",
    "--out", "src/bindings.rs",
    "--filter", "Microsoft.Windows.Widgets.Providers",
    "--implement",
]);
```

Two things that cost time and are worth writing down:

- **Filter to `.Providers`, not `Microsoft.Windows.Widgets`.** The wider filter
  drags in the `Feeds.Providers` namespace, whose `Headers` properties return
  `Windows.Foundation.Collections.StringMap` — a type `windows-collections 0.3`
  does not export. Four unresolvable errors, all of them in code glint never
  calls. Narrowing the filter drops the namespace and the problem.
- **Do not reach for `windows-bindgen 0.100`.** It needs rustc 1.95 and this
  box has 1.92, and it would force the whole project onto `windows-core 0.100`.
  0.66 has no such constraint and matches what the tray app already links.

The installed runtime here is `WindowsAppRuntime.1.8`. The Widgets Providers
API has existed since App SDK 1.2, so 1.8 is enough; the walkthrough's "2.3.1
or later" is for newer features (web widgets, customization) that this does not
use. Pin 1.8 and avoid deploying a second runtime.

Bindings get checked in as generated output and refreshed by a tool, which is
how `windows-rs` itself handles the WinUI winmd set.

## Spike 2 — run this before writing the provider

Five risks, each with an exit criterion. This is the same shape as Spike 1,
which is what saved the NPU work from being built on DXCore.

| Risk | Question | Exit criterion | Status |
|------|----------|----------------|--------|
| **W1** | Can `windows-bindgen` produce usable bindings for `Microsoft.Windows.Widgets.Providers`, including `#[implement]` for `IWidgetProvider`? | A Rust exe that compiles and calls `WidgetManager::GetDefault()` without panicking | **PASS** |
| **W2** | Does the Widgets host activate a Rust-authored OOP COM server from an MSIX package? | `CreateWidget` fires and writes to a log file after pinning the widget | open |
| **W3** | Does the board render `data:image/png;base64,...` in an `Image`? | The bar graphic appears on a pinned card | open |
| **W4** | What update cadence does the host actually honour between `Activate` and `Deactivate`? | A measured number: push at 1 s, 2 s and 5 s, count what lands | open |
| **W5** | Can the package be signed and deployed here without a Store account, and does the packaged tray app still behave? | A self-signed cert in Trusted People, `Add-AppxPackage` succeeds, the widget appears in the picker, **and** the tray icon still opens the window and saves a dragged position | open |

### Spike 2 findings — W1 (2026-10-08)

W1 was the risk that could have killed the Rust approach. It does not.

Four results, in order, from a throwaway crate against the installed runtime:

```
1. IWidgetProvider implemented in Rust: IWidgetProvider(0x1f1978d85f8)
2. WidgetManager::GetDefault -> Class not registered (0x80040154)
3. DllGetActivationFactory OK: IActivationFactory(0x1f1978e87b0)
4. QI IWidgetManagerStatics OK: IWidgetManagerStatics(0x1f1978e87b8)
```

1. **`#[implement(IWidgetProvider)]` compiles.** All six methods —
   `CreateWidget`, `DeleteWidget`, `OnActionInvoked`,
   `OnWidgetContextChanged`, `Activate`, `Deactivate` — with the signatures
   the host expects. The object constructs and vends a real interface pointer.
2. **`GetDefault()` fails `REGDB_E_CLASSNOTREG`, and that is correct.** An
   unpackaged exe has no package identity, so `RoGetActivationFactory` cannot
   resolve the type through a package graph it is not part of. This is the
   expected unpackaged result, not a binding defect — which results 3 and 4
   prove.
3. **Loading `Microsoft.Windows.Widgets.dll` from the framework package and
   calling `DllGetActivationFactory` returns a live factory.** The type is real
   and reachable; only the registration was missing.
4. **`QueryInterface` for `IWidgetManagerStatics` succeeds.** This is the
   result that matters most: the generated IID and vtable layout match the
   shipped implementation byte for byte. Had `windows-bindgen` produced
   anything subtly wrong, the QI would have failed here rather than at runtime
   inside the board.

So the MSIX supplies the one missing piece — identity — and everything else is
already proven. Two prior worries are now dead: the archived `windows-app`
crate's "too tied to .NET and Visual Studio" verdict was about the
**bootstrapper**, which a packaged app never calls; and `windows-bindgen` issue
#4983 (third-party winmd referencing Win32 types) does not apply, because these
bindings reference no Win32 types at all — only `windows-core`.

The C# fallback provider is therefore **off the table** unless W2 surprises us.

**W3 has a fallback chain**, so it cannot block the project, only the fidelity
of the bars:

1. Data URI PNG — the exact design, bars drawn by the same GDI code the panel
   uses, encoded inline.
2. Stacked `Container`s with pixel `minHeight` and a `style` colour —
   approximate, native, no image.
3. Unicode block characters in a `TextBlock` — crude, certain to work.

## Phase B spec impact (provisional)

Requirements get written **after** the spike, for the reason Spike 1 proved:
the v1 plan asserted DXCore would find the NPU and it did not, so a requirement
written first would have been wrong. Expect roughly:

| ID | Requirement | Verification |
|----|-------------|--------------|
| GLINT-WIDGET-PROVIDER | The app registers a Widgets Board provider and the widget appears in the picker | manual |
| GLINT-WIDGET-CARD | The card shows a letter and a bar per measurement, with numbers at medium and large | manual |
| GLINT-WIDGET-CADENCE | The provider updates only between `Activate` and `Deactivate` | manual |
| GLINT-PACKAGE | The MSIX carries both executables and declares both framework dependencies | manual |

`GLINT-WIDGET-CADENCE` is the only candidate for a `test`, and it probably does
not earn one either: asserting "no update while deactivated" needs the real
host, which is not a unit test. Likely `manual` with a logged trace.

## Risks beyond the spike

| Risk | Handling |
|------|----------|
| Packaging changes how "Start with Windows" should work | A packaged app should use `uap5:StartupTask` rather than the HKCU `Run` value. GLINT-STARTUP needs amending; the current code still works but is the wrong idiom in a package |
| MSIX changes the double-click-the-exe story in the README | The package is the **full** product: it carries the tray app and the provider, with a Start menu entry for the former. The loose exe stays as a lighter option that gives the window without the widget. Neither distribution is widget-only |
| Packaging may move `%APPDATA%\Glint\config.json` | Desktop Bridge redirects some `AppData` writes for packaged full-trust apps. If it moves, the packaged build starts at defaults once: re-drag the window, re-tick the menu, done. No migration code — single user, one-time cost |
| Two processes both sampling PDH | Only while the board is open, at 0.43 ms a tick. Measure private bytes of the provider process and hold it to the same bar as the tray app |
| The generated bindings drift when the App SDK updates | Pin the version in the refresh tool, as `windows-rs` does for WinUI |

---

## Verification

Phase A: `cargo test` for the extended `config_parse` plus the untouched
`luid_parse`, then walk the three new manual procedures.

Phase B: the five spike exit criteria first, then the manual procedures that
the spike's findings justify writing.

## Sources

- [Implement a widget provider in a win32 app (C++/WinRT)](https://learn.microsoft.com/en-us/windows/apps/develop/widgets/implement-widget-provider-win32)
- [Widget provider package manifest XML format](https://learn.microsoft.com/en-us/windows/apps/develop/widgets/widget-provider-manifest)
- [Widget providers](https://learn.microsoft.com/en-us/windows/apps/develop/widgets/widget-providers)
- [WidgetManager.UpdateWidget](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.windows.widgets.providers.widgetmanager.updatewidget)
- [Windows App SDK deployment architecture](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/deployment-architecture)
- [MddBootstrapInitialize2](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/win32/mddbootstrap/nf-mddbootstrap-mddbootstrapinitialize2)
- [Adaptive Cards Image element](https://learn.microsoft.com/en-us/adaptive-cards/schema-explorer/image)
- [microsoft/windows-rs](https://github.com/microsoft/windows-rs)
- [microsoft/windows-app-rs (archived)](https://github.com/microsoft/windows-app-rs)
