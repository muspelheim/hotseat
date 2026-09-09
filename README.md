# hotseat

Hand one monitor between the machines on your desk, by hotkey, without reaching
for the monitor's buttons.

**Status: early but working. Milestone 2 — discovery, config and switching,
verified end to end between a Mac and a Linux box on one panel. No daemon and
no peer mesh yet, so a hotkey means binding `hotseat give` in the hotkey tool
you already use.**

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

## The case that motivated all of this

The reference monitor is a Samsung Odyssey G5 G52A. Here is what it took to
work out how to switch it, and why each layer of guessing failed:

**Its capabilities string lies.** It declares `60(01 03)` — VCP `0x60` supports
only `01` (VGA-1) and `03` (DVI-1). The panel has exactly one HDMI 2.0 and one
DisplayPort 1.2, and no analogue or DVI input at all.

**Its write codes are mixed.** DisplayPort-1 is `15`, the standard MCCS value.
HDMI-1 is `5` — a vendor value that MCCS calls *Composite-1*. The standard
HDMI-1 code, `17`, does nothing whatsoever.

**Its read-back uses a third convention.** Reading `0x60` returns `5` when HDMI
is displayed and `6` when DisplayPort is. So `5` means HDMI both to read and to
write, but DisplayPort is `6` to read and `15` to write.

Every one of those was measured, and each broke a plausible shortcut:

| Shortcut | Why it fails here |
|---|---|
| Trust the declared values | Neither working value is declared |
| Trust the MCCS table | `17` is inert; the working HDMI code is `5` |
| Read the current input and toggle | Read and write conventions disagree |
| Assume a vendor table | This panel mixes MCCS and vendor codes |

The full string is at [`tests/fixtures/odyssey-g52a.caps`](tests/fixtures/odyssey-g52a.caps)
with a regression test. A monitor that misbehaves makes a better fixture than
one that behaves.

## What it discovers

**Which input values are worth believing, in three tiers that are never
collapsed:**

| Tier | Meaning |
|---|---|
| `verified` | Confirmed by actually switching the display. The only real authority. |
| `declared` | The display listed it in its capabilities string. A hint, nothing more. |
| `standard` | From the MCCS table. A well-founded assumption. |

Candidates are the **union** of all three, never narrowed. On the reference
panel, narrowing to the declaration would have discarded both working values.

**Whether this link's DDC reads can be believed.** DDC writes are
fire-and-forget over I²C; reads need the display to answer. Some links carry
writes perfectly while answering every read with the same meaningless number —
measured via `m1ddc` on an Apple Silicon HDMI link, which returned `110` for
brightness, contrast, volume and input source alike. `hotseat probe` reads
several unrelated VCP codes and refuses to trust reads that agree when they
have no business agreeing.

**Which panel it is talking to**, well enough to store settings against. Config
is keyed on the monitor, not on a backend handle that changes when you move a
cable:

| Tier | Key | Notes |
|---|---|---|
| `Edid` | `SAM:7181:H4ZT300577` | Vendor, model and serial. Distinguishes two identical panels. |
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
  default. Without access hotseat finds no displays at all — and that failure
  looks identical to having no monitor attached, so `hotseat` now diagnoses it
  explicitly rather than listing it as one possibility among several.
- **`pkg-config` and libudev headers** to build. `ddc-hi` reaches libudev via
  `ddc-i2c` → `i2c-linux` → `udev` → `libudev-sys`, and without them the build
  dies with a bare pkg-config error that never mentions udev.

Group membership applies only to sessions started after it was granted, so open
a fresh login afterwards. The installer tells you which mechanism your distro
actually has, since Ubuntu 26.04 ships neither `newgrp` nor `sg`.

On macOS none of this applies: DDC needs no privileged setup at all.

### Why piping to a shell is safe here

Every statement in `install.sh` lives inside a function, and `main` runs on the
very last line. A download truncated mid-transfer therefore defines a few
functions and does nothing — verified by cutting the file at 30%, 60% and 90%,
where bash refuses to execute any of it. A plain top-to-bottom script would
instead run whatever prefix happened to arrive, which on a dropped connection
means a half-configured machine. CI enforces this.

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
hotseat config own-input 5         # this machine's input
hotseat config peer linux-pc 15    # where the other machine is
hotseat config show
```

Then switch:

```sh
hotseat give linux-pc              # hand the monitor over
hotseat give linux-pc --dry-run    # resolve it without writing
hotseat take                       # reclaim it
hotseat set-input 15 --yes         # raw escape hatch
```

Record what you confirm, and retract what turns out to be wrong:

```sh
hotseat config verified 5          # this value really does switch it
hotseat config unverify 17         # ...and this one provably does not
hotseat config can-pull true       # this machine can reclaim the monitor
hotseat config forget-peer old-pc  # machine is gone
```

Values are described by the machine that lives on them rather than by the MCCS
table, because on a vendor-coded panel the table is simply wrong — `5` is
Composite-1 to MCCS and HDMI-1 in reality.

### Binding a hotkey

hotseat has no daemon yet, so bind it in whatever you already run. It generates
the snippet with the **absolute** path to its own binary:

```sh
hotseat hotkey linux-pc                      # skhd on macOS
hotseat hotkey linux-pc --flavour autohotkey # Windows
hotseat hotkey linux-pc --flavour command    # bare line for any GUI editor
```

The absolute path is not fussiness. A hand-built version of this setup failed
silently because a launch agent invoked a bare command name, and launchd's `PATH`
is only `/usr/bin:/bin:/usr/sbin:/sbin` — no Homebrew, no `~/.local/bin`. The
hotkey did nothing, with no error anywhere. `hotseat` is verified to run under
`env -i` with neither `HOME` nor `PATH` set.

A headless machine has nothing listening for keystrokes, so there a hotkey is
not possible at all — run the command instead.

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
promise is *one guided setup, then nothing* — better than the status quo, but
not magic.

## Known limitations

- **A successful write is not proof.** `give` reports "write accepted", never
  "switched", because DDC cannot tell the difference and panels do accept writes
  they then ignore. Where a panel's `0x60` read-back tracks the selected input,
  hotseat could confirm its own writes automatically — that is the next thing
  worth building.
- **Linux binaries are dynamically linked against libudev**, so they are not
  drop-anywhere static executables. `ddc-hi` turns on `ddc-i2c`'s udev-based
  enumeration unconditionally and Cargo features are additive, so it cannot be
  disabled downstream. Any distro with systemd/udev already has the library.
- **`ddc_macos::Monitor::edid()` returns nothing** on the hardware this was built
  against, so the `Edid` key tier is unreachable there and the `Serial` tier is
  used instead. The EDID parser exists and is unit-tested but is not exercised
  on that machine.
- **The same panel can produce different keys on different machines.** Measured:
  macOS falls back to `serial:1129919028` while Linux, which does expose EDID,
  produces `SAM:7181:H4ZT300577`. The serial agrees across both, so the mesh
  will have to match panels on that rather than on the whole key.
- **Enumeration is empty while a display sleeps.** A locked screen or a
  power-saved panel yields zero DDC displays even though the OS still lists the
  monitor. hotseat distinguishes asleep from absent in its output but cannot act
  on a sleeping display.
- **No mesh yet**, so peers are configured by hand on each machine.

## Roadmap

- **M1** — read-only discovery. *Done.*
- **M2** — switching, config persistence, verified input codes, hotkey snippets.
  *Done, and verified end to end between two machines on one panel.*
- **M3** — automatic verification. Where a panel's read-back tracks the selected
  input, hotseat can write, read back, and confirm the switch on its own. That
  turns "verified" from something a human asserts into something measured, and
  is what self-configuration should have meant from the start.
- **M4** — LAN peer mesh over mDNS, so one hotkey works from any machine.
  **Note this is a convenience feature, not a necessity.** It was originally
  justified by the belief that panels only accept input changes from the machine
  they are currently displaying, so a machine that gave the monitor away could
  never take it back. That turned out to be false: the appearance of it was
  caused entirely by writing `17`, a value this panel ignores. With the correct
  code, both machines reclaim the monitor perfectly well while inactive. The
  mesh is still worth building for topology discovery and single-hotkey
  operation from any machine — but not for that reason.
- **M5** — own hotkey daemon and service installers, Linux polish, packaging.

## Licence

MIT.

[display-switch]: https://github.com/haimgel/display-switch
[monitor-switch]: https://github.com/mjkoo/monitor-switch
[display-input-switcher]: https://github.com/3urobeat/display-input-switcher
[ddcswitch]: https://github.com/markdwags/ddcswitch
[`ddc-hi`]: https://github.com/arcnmx/ddc-hi-rs
[`global-hotkey`]: https://crates.io/crates/global-hotkey
