# Capabilities fixtures

Raw MCCS capabilities strings captured from real hardware with `hotseat caps`.

## odyssey-g52a.caps

Samsung Odyssey G5 G52A 27", captured 2026-09-09 over an HDMI link from an
Apple M4 Max. Byte-identical when captured from Linux over DisplayPort, so the
misreporting below is the panel's, not the platform's.

This monitor is the project's reference device precisely because it is wrong in
three different ways at once, which makes it far more valuable than a
well-behaved panel.

**It declares inputs it does not have.** `60(01 03)` claims VCP `0x60` supports
only `01` (VGA-1) and `03` (DVI-1). The panel has one HDMI 2.0 and one
DisplayPort 1.2, and no analogue or DVI input at all.

**Its write codes are mixed.** Measured by writing each candidate and confirming
the result:

| Input | Write value | Note |
|---|---|---|
| DisplayPort-1 | `15` | the standard MCCS value |
| HDMI-1 | `5` | a vendor value; MCCS calls this Composite-1 |

MCCS `17`, the standard HDMI-1 code, does nothing at all. So do `1` and `3`,
the two values the panel itself declares.

**Its read-back uses a third convention.** Reading `0x60` returns `5` while HDMI
is displayed and `6` while DisplayPort is. HDMI is therefore `5` both to read
and to write, while DisplayPort is `6` to read but `15` to write.

That read-back does track the selected input, which makes it usable for
confirming a switch — but only after establishing the mapping by experiment. It
cannot be assumed from any table.

**`model(FALCON)`** is an internal codename, not the product name. Anything that
labels displays from that field shows users "FALCON".

Any change that would cause `inputs::candidates` to drop `15` or `5` for this
display is a regression. `capabilities_are_a_signal_not_the_truth` exists to
catch it: both survive only because candidates are a union of the standard
table with whatever was declared, and `5` is present only because the standard
table happens to include it under a different name.
