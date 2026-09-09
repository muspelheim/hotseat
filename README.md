# hotseat

Hand one monitor between the machines on your desk, by hotkey, without reaching
for the monitor's buttons.

**Status: early. Milestone 1 only — every command here is read-only and writes
nothing to your displays.**

## Why another one of these

There is no shortage of DDC/CI input switchers: [display-switch], [monitor-switch],
[display-input-switcher], [ddcswitch], and `ddcutil` itself. They work, and if one
of them already suits you, use it.

They share one assumption, though: that you will tell them which input each
machine occupies, and that your monitor's input codes are the standard ones. In
practice both parts can be wrong in ways that are genuinely hard to debug,
because a DDC write that goes nowhere produces no error.

hotseat's premise is that **the discovery is the hard part**, so it should be the
part that gets automated.

## What it discovers

**Whether this link's DDC reads can be believed at all.** DDC writes are
fire-and-forget over I²C; reads need the display to answer. Some links carry
writes perfectly while answering every read with the same meaningless number.
Measured case: an M4 Max reaching a Samsung Odyssey G52A over HDMI *via m1ddc*
returned `110` for brightness, contrast, volume and input source alike, while
writes worked flawlessly. `hotseat probe` reads several unrelated VCP codes and
refuses to trust reads that agree when they have no business agreeing.

This matters because the obvious design — read the current input, then toggle —
silently does the wrong thing on such a link.

**Which input values the display might actually accept, and how much each is
worth believing.** Three tiers, never collapsed into one:

| Tier | Meaning |
|---|---|
| `verified` | Confirmed by actually switching the display. The only real authority. |
| `declared` | The display listed it in its capabilities string. A hint, nothing more. |
| `standard` | From the MCCS table. A well-founded assumption. |

That middle tier is deliberately weak, because displays misreport themselves.
The reference monitor for this project declares:

```
60(01 03)
```

…meaning VCP `0x60` supports only `01` (VGA-1) and `03` (DVI-1). The panel has
exactly one HDMI 2.0 and one DisplayPort 1.2, and no analogue or DVI input at
all. The two values that *do* switch it, `15` and `17`, appear nowhere in its own
declaration. A tool that narrowed to what this display claims would discard the
only values that work.

The full string is kept at [`tests/fixtures/odyssey-g52a.caps`](tests/fixtures/odyssey-g52a.caps)
and a regression test asserts that `15` and `17` survive it. A monitor that
misbehaves makes a better fixture than one that behaves.

## Install

Requires a Rust toolchain.

```sh
git clone https://github.com/muspelheim/hotseat
cd hotseat
cargo build --release
```

## Use

```sh
hotseat probe     # full read-only report per display
hotseat status    # one line per display
hotseat caps      # raw MCCS capabilities string, verbatim
```

`hotseat caps` output is the single most useful thing to attach to a bug report.

## Platform support

hotseat talks DDC through [`ddc-hi`], which covers all three platforms. Global
hotkeys are a different story, and the gaps are upstream rather than fixable
here.

| Platform | DDC | Global hotkey | One-time setup |
|---|---|---|---|
| Windows | `ddc-winapi` / `nvapi` | native | none |
| macOS | `ddc-macos` (IOAVService) | native | Accessibility permission |
| Linux + X11 | `ddc-i2c` | native | `i2c-dev` loaded, `/dev/i2c-*` readable |
| Linux + Wayland | `ddc-i2c` | **none** | as X11; bind `hotseat take` in your DE |

**hotseat will not claim zero configuration.** macOS Accessibility is a TCC
permission whose database is SIP-protected, so no installer can grant it.
Wayland has no global-shortcut mechanism available here at all — the
[`global-hotkey`] crate does not support it and `xdg-desktop-portal-wlr` ships no
`GlobalShortcuts` implementation. Linux i2c access needs root once. The honest
promise is *one guided setup, then nothing* — which is still better than the
status quo, but it is not magic.

## Known limitations

- **No EDID on macOS.** `ddc-macos` exposes no EDID fields, so `MonitorId::key`
  returns `None` there and config cannot yet be keyed on panel identity. Display
  names fall back to the backend handle, which is fine for showing a human and
  not fine as a key — two identical monitors would collide. Reading EDID
  ourselves via IOKit is the fix.
- **Enumeration is empty while a display sleeps.** A locked screen or a
  power-saved panel yields zero DDC displays even though the OS still lists the
  monitor. Callers must distinguish "asleep" from "absent" and retry.
- **Read-back and write encodings can differ.** The reference panel reads `0x60`
  back as `5` while accepting `15`/`17` for writes. `hotseat probe` flags this
  when it detects it. Never feed a read-back value straight back as a write.

## Roadmap

- **M1** — read-only discovery. *Current.*
- **M2** — switching, config persistence, verified input codes, service install.
- **M3** — LAN peer mesh over mDNS, so one hotkey works from any machine. This
  exists because of a physical constraint, not for its own sake: many panels
  accept an input change only from the machine currently being displayed, so a
  machine that has handed the monitor away cannot take it back and has to ask.
- **M4** — hotkeys, Linux polish, packaging.

## Licence

MIT.

[display-switch]: https://github.com/haimgel/display-switch
[monitor-switch]: https://github.com/mjkoo/monitor-switch
[display-input-switcher]: https://github.com/3urobeat/display-input-switcher
[ddcswitch]: https://github.com/markdwags/ddcswitch
[`ddc-hi`]: https://github.com/arcnmx/ddc-hi-rs
[`global-hotkey`]: https://crates.io/crates/global-hotkey
