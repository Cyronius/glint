# glint

A tray system monitor for Windows. The tray icon opens a small borderless
window with CPU, memory, GPU, NPU and disk statistics. The window stays
open and on top until it is minimized. A hidden window costs almost nothing.

![Glint window](docs/screenshot.png)

## Build and run

```
cargo build --release
target\release\glint.exe
```

The release binary is about 195 KB. Private memory stays near 6.5 MB.

The window opens in the lower right corner, clear of the taskbar.

| Action | Result |
|--------|--------|
| Left click the tray icon | Show the window, or hide it when it is open |
| Right click the tray icon | Start with Windows, Reset position, Exit |
| Minimize button | Hide the window; hover for a tooltip |
| Drag the header | Move the window; the position is remembered |

There is no taskbar button and no title bar, so **Exit** in the right click
menu is how to quit.

Settings live in `%APPDATA%\glint\config.json`.

## Probe binaries

| Command | What it answers |
|---------|-----------------|
| `cargo run --release --bin spike` | Adapter discovery, PDH cost, CPU base clock |
| `cargo run --release --bin spike -- --watch` | A 30 s live table, to compare with Task Manager |
| `cargo run --release --bin wildcard` | Whether a PDH wildcard counter re-expands |
| `cargo run --release --bin sample` | The sampler, as console output |
| `cargo run --release --bin cpucheck` | How `% Processor Utility` reacts to clock boost |

## Tests

```
cargo test
```

Two requirements earn automated tests: `GLINT-LUID-PARSE` and `GLINT-CONFIG-PARSE`.
Every other requirement carries a manual procedure in the spec.

## Spec

`specs/glint/spec.md` is authoritative. It holds the requirements, the
manual procedures, and the design notes that explain why the app uses WDDM
rather than DXCore, and GDI rather than Direct2D.

## Requirements

No external crates beyond `windows`. Windows 10 or later. Rust stable, MSVC
target. The NPU row needs WDDM 2.9 for `D3DKMTEnumAdapters3`; without it the
row is simply absent.
