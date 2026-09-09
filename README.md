# hotseat

Hand one monitor between the machines on your desk, by hotkey, without reaching
for the monitor's buttons.

**Status: early but usable. Milestone 2 — discovery, config and switching. No
daemon and no peer mesh yet, so a hotkey means binding `hotseat give` in the
hotkey tool you already use.**

## Why another one of these

There is no shortage of DDC/CI input switchers: [display-switch], [monitor-switch],
[display-input-switcher], [ddcswitch], and `ddcutil` itself. They work, and if one
of them already suits you, use it.

They share one assumption, though: that you will tell them which input each
machine occupies, and that your monitor's input codes are the standard ones. In
practice both parts can be wrong in ways that are genuinely hard to debug,
because a DDC write that goes nowhere produces no error at all.

hotseat's premise is that **the discovery is the hard part**, so it should be the
part that gets automated — and that anything it has not actually measured should
say so.

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

**Which panel it is talking to, well enough to store settings against.** Config
is keyed on the monitor, not on a backend handle that changes when you move a
cable. Three tiers again, because backends differ in what they expose:

| Tier | Key | Notes |
|---|---|---|
| `Edid` | `SAM:7180:H4ZT300577` | Vendor, model and serial. Distinguishes two identical panels. |
| `Serial` | `serial:1129919028` | Serial only. Still one physical panel. |
| `CapabilitiesFingerprint` | `caps:85944171f73967e8` | FNV-1a of the capabilities string. Works everywhere, including Windows where WinAPI exposes no EDID — but two identical monitors share it, and hotseat says so. |

## Install

One command, Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/muspelheim/hotseat/main/install.sh | bash
```

With options, note the `-s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/muspelheim/hotseat/main/install.sh | bash -s -- --dry-run
```

`--dry-run` prints every command it would run and changes nothing. `--no-build`
does the system setup only, `--prefix DIR` chooses where the binary goes
(default `~/.local/bin`), and `--ref REF` pins a git ref.

It detects the platform, installs what is missing, fetches a Rust toolchain if
you have none, clones the source, builds, installs, and verifies. It is
idempotent, refuses to run as root, uses `sudo` only where required, and prints
how to undo the privileged parts. From an existing clone, `./install.sh` does
the same thing without cloning again.

**It builds from source**, so a first install pulls a toolchain and compiles —
minutes, not seconds. Prebuilt binaries are an M4 item.

### What it does on Linux, and why

Linux needs two things macOS and Windows do not:

- **`/dev/i2c-*` readable by your user**, with `i2c-dev` available. DDC/CI
  reaches the monitor over the GPU's I2C bus, and those nodes are root-only by
  default. Without access hotseat finds no displays at all, and the failure
  looks identical to having no monitor attached.
- **`pkg-config` and libudev headers** to build. `ddc-hi` reaches libudev via
  `ddc-i2c` → `i2c-linux` → `udev` → `libudev-sys`, and without them the build
  dies with a bare pkg-config error that never mentions udev.

Group membership does not apply to shells that were already open, so start a
fresh session afterwards or run `newgrp i2c`.

On macOS none of this applies: DDC needs no privileged setup at all.

### Why piping to a shell is safe here

Every statement in `install.sh` lives inside a function, and `main` runs on the
very last line. A download truncated mid-transfer therefore defines a few
functions and does nothing — verified by cutting the file at 30%, 60% and 90%,
where bash refuses to execute any of it. A plain top-to-bottom script would
instead run whatever prefix happened to arrive, which on a dropped connection
means a half-configured machine.

For the same reason it downloads rustup to a file and then runs it, rather than
piping the network into a shell a second time.

## Use

Look first. Nothing here writes to a display:

```sh
hotseat probe     # full report per display
hotseat status    # one line per display
hotseat caps      # raw MCCS capabilities string, verbatim
```

`hotseat caps` output is the single most useful thing to attach to a bug report.

Then tell it your layout — `hotseat probe` lists the plausible values:

```sh
hotseat config own-input 17            # this machine is on HDMI-1
hotseat config peer win-desktop 15     # the PC is on DisplayPort-1
hotseat config show
```

Then switch:

```sh
hotseat give win-desktop               # hand the monitor over
hotseat give win-desktop --dry-run     # resolve it without writing
hotseat take                           # reclaim it, where the panel allows
hotseat set-input 15 --yes             # raw escape hatch
```

Record what you observe, so hotseat stops guessing:

```sh
hotseat measure-pull win-desktop --yes # disruptive; reports, does not guess
hotseat config verified 15             # this value really does switch it
hotseat config can-pull false          # panel ignores machines it isn't showing
```

Once `can-pull` is `false`, `hotseat take` refuses with an explanation instead of
writing a value the panel would silently ignore.

### Binding a hotkey

hotseat has no daemon yet, so bind it in whatever you already run. It generates
the snippet with the **absolute** path to its own binary:

```sh
hotseat hotkey win-desktop                      # skhd on macOS
hotseat hotkey win-desktop --flavour autohotkey # Windows
hotseat hotkey win-desktop --flavour command    # bare line for any GUI editor
```

The absolute path is not fussiness. A hand-built version of this setup failed
silently because a launch agent invoked a bare command name, and launchd's `PATH`
is only `/usr/bin:/bin:/usr/sbin:/sbin` — no Homebrew, no `~/.local/bin`. The
hotkey did nothing, with no error anywhere. `hotseat` is verified to run under
`env -i` with neither `HOME` nor `PATH` set.

## Platform support

hotseat talks DDC through [`ddc-hi`], which covers all three platforms. Global
hotkeys are a different story, and the gaps are upstream rather than fixable
here.

| Platform | DDC | Global hotkey | One-time setup |
|---|---|---|---|
| Windows | `ddc-winapi` / `nvapi` | via AutoHotkey | none |
| macOS | `ddc-macos` (IOAVService) | via skhd or similar | that tool's Accessibility permission |
| Linux + X11 | `ddc-i2c` | via your DE | `i2c-dev` loaded, `/dev/i2c-*` readable |
| Linux + Wayland | `ddc-i2c` | via your DE | as X11 |

**hotseat will not claim zero configuration.** macOS Accessibility is a TCC
permission whose database is SIP-protected, so no installer can grant it.
Wayland has no global-shortcut mechanism available here at all — the
[`global-hotkey`] crate does not support it and `xdg-desktop-portal-wlr` ships no
`GlobalShortcuts` implementation. Linux i2c access needs root once. The honest
promise is *one guided setup, then nothing* — which is still better than the
status quo, but it is not magic.

## Known limitations

- **`ddc_macos::Monitor::edid()` returns nothing** on the hardware this was built
  against, so the `Edid` key tier is unreachable there and the `Serial` tier is
  used instead. The EDID parser exists and is unit-tested, but is not exercised
  on that machine.
- **Enumeration is empty while a display sleeps.** A locked screen or a
  power-saved panel yields zero DDC displays even though the OS still lists the
  monitor. hotseat distinguishes "asleep" from "absent" in its output, but
  cannot act on a sleeping display.
- **Read-back and write encodings can differ.** The reference panel reads `0x60`
  back as `5` while accepting `15`/`17` for writes. `hotseat probe` flags this
  when it detects it. Never feed a read-back value straight back as a write.
- **A successful write is not proof.** `give` reports "write accepted", never
  "switched", because DDC cannot tell the difference and this panel accepts
  writes it then ignores.
- **No mesh yet**, so peers are configured by hand on each machine.
- **Linux binaries are dynamically linked against libudev**, so they are not
  drop-anywhere static executables. `ddc-hi` turns on `ddc-i2c`'s udev-based
  enumeration unconditionally and Cargo features are additive, so it cannot be
  disabled downstream. Any distro with systemd/udev already has the runtime
  library.
- **The same panel can produce different config keys on different machines.**
  Measured: macOS reported EDID model `0x7180` for the reference monitor while
  the Linux EDID reports `0x7181`, and macOS exposes no EDID at all so it falls
  back to the serial tier. The serial agrees across both. Harmless today, since
  config is per-machine, but the M3 mesh will have to match panels on the serial
  rather than on the whole key.

## Roadmap

- **M1** — read-only discovery. *Done.*
- **M2** — switching, config persistence, verified input codes, hotkey snippets.
  *Current.*
- **M3** — LAN peer mesh over mDNS, so one hotkey works from any machine. This
  exists because of a physical constraint, not for its own sake: many panels
  accept an input change only from the machine currently being displayed, so a
  machine that has handed the monitor away cannot take it back and has to ask.
  That is the `can_pull = false` case, and it is the norm rather than the
  exception.
- **M4** — own hotkey daemon and service installers, Linux polish, packaging.

## Licence

MIT.

[display-switch]: https://github.com/haimgel/display-switch
[monitor-switch]: https://github.com/mjkoo/monitor-switch
[display-input-switcher]: https://github.com/3urobeat/display-input-switcher
[ddcswitch]: https://github.com/markdwags/ddcswitch
[`ddc-hi`]: https://github.com/arcnmx/ddc-hi-rs
[`global-hotkey`]: https://crates.io/crates/global-hotkey
