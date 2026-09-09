# Capabilities fixtures

Raw MCCS capabilities strings captured from real hardware with `hotseat caps`.

## odyssey-g52a.caps

Samsung Odyssey G5 G52A 27", captured 2026-09-09 over an HDMI link from an
Apple M4 Max.

This monitor is the project's reference device precisely because it
**misreports itself**, which makes it far more valuable than a well-behaved
panel:

- `60(01 03)` — it declares that VCP 0x60 accepts only `01` (VGA-1) and `03`
  (DVI-1). The panel physically has one HDMI 2.0 and one DisplayPort 1.2 and no
  analogue or DVI input at all. Values `15` (DisplayPort-1) and `17` (HDMI-1)
  were verified by hand to switch it successfully, and **neither appears in its
  own declaration**.
- `model(FALCON)` — an internal codename, not the product name. Anything that
  labels displays from this field shows users "FALCON".
- Reading `0x60` on this panel returns `5`, while writes accept `15`/`17`.
  Read-back and write values are in different encodings.

Any change that would cause `inputs::candidates` to exclude `15` or `17` for
this display is a regression, and `capabilities_are_a_signal_not_the_truth`
exists to catch it.
