//! hotseat CLI.
//!
//! Milestone 1 provides only read-only commands. Nothing here writes to a
//! display, so it is safe to run against a machine you are actively using.

use anyhow::Result;
use clap::{Parser, Subcommand};

use hotseat::inputs::{self, INPUT_SELECT};
use hotseat::monitor::MonitorId;
use hotseat::report::DisplayReport;
use hotseat::trust::{self, ReadTrust};
use hotseat::vcp::Vcp;

#[derive(Parser)]
#[command(
    name = "hotseat",
    about = "Hand one monitor between the machines on your desk",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect attached displays without writing to them.
    ///
    /// Reports EDID identity, whether this link's DDC reads can be believed,
    /// and which input values the display might accept.
    Probe,
    /// One-line summary per display.
    Status,
    /// Dump each display's raw MCCS capabilities string verbatim.
    ///
    /// Useful in bug reports, and the raw text is often the only way to see
    /// that a display is misreporting itself.
    Caps,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Probe => probe(),
        Command::Status => status(),
        Command::Caps => caps(),
    }
}

/// Gather a full read-only report for every attached display.
fn collect() -> Vec<DisplayReport> {
    ddc_hi::Display::enumerate()
        .into_iter()
        .map(|mut display| {
            let backend = display.info.backend.to_string();
            let backend_id = display.info.id.clone();

            // Capture identity BEFORE touching capabilities. `update_capabilities`
            // overwrites `info.model_name` with the capabilities string's model
            // field, which on the reference G52A is the internal codename
            // "FALCON" rather than "Odyssey G52A". Reading identity first keeps
            // the name a human would recognise.
            let id = MonitorId::from_info(&display.info);
            let product_name = display.info.model_name.clone();

            // Fetch the raw string ourselves so the reported byte count is the
            // capabilities string and not something else wearing its label.
            let capabilities = {
                let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
                vcp.capabilities_raw()
                    .map(|bytes| bytes.len())
                    .map_err(|e| e.to_string())
            };
            // Populate info.mccs_database. Failure is already reflected above.
            let _ = display.update_capabilities();

            // Surface the codename only when it actually disagrees.
            let declared_model = display
                .info
                .model_name
                .clone()
                .filter(|m| Some(m) != product_name.as_ref());

            let has_edid = display.info.edid_data.is_some();
            let declared_inputs: Vec<u8> = inputs::declared(&display.info.mccs_database)
                .into_iter()
                .map(|(c, _)| c)
                .collect();
            // No verified values yet: verification lands in M2 with persistence.
            let candidates = inputs::candidates(&display.info.mccs_database, &[]);

            let (samples, trust) = {
                let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
                trust::evaluate(&mut vcp)
            };

            // Only report a current input when the link has earned it.
            let current_input = if trust.is_trusted() {
                let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
                vcp.get(INPUT_SELECT).ok().map(|r| r.value as u8)
            } else {
                None
            };

            DisplayReport {
                backend,
                backend_id,
                id,
                declared_model,
                has_edid,
                capabilities,
                samples,
                trust,
                candidates,
                declared_inputs,
                current_input,
            }
        })
        .collect()
}

/// Explain an empty enumeration without guessing at the cause.
fn explain_no_displays() {
    println!("No DDC-capable displays found.");
    println!();
    println!("This is not the same as having no monitor attached. Common causes:");
    println!();
    println!("  * The display is asleep or the screen is locked. DDC enumeration");
    println!("    returns nothing while a panel is dark, even though the OS still");
    println!("    lists it. Wake the display and try again.");
    if cfg!(target_os = "linux") {
        println!("  * The i2c-dev module is not loaded, or /dev/i2c-* is not readable");
        println!("    by your user.");
    }
    if cfg!(target_os = "macos") {
        println!("  * The link does not carry DDC at all. Notably, m1ddc and some");
        println!("    other tools cannot reach displays behind the built-in HDMI port");
        println!("    of M1 and entry-level M2 Macs.");
    }
    println!("  * The monitor is currently showing a different input.");
}

fn probe() -> Result<()> {
    let reports = collect();
    if reports.is_empty() {
        explain_no_displays();
        return Ok(());
    }

    println!("hotseat probe - read-only, nothing was written to any display\n");
    for (i, r) in reports.iter().enumerate() {
        println!("[{}] {}", i + 1, r.render());
    }
    Ok(())
}

fn status() -> Result<()> {
    let reports = collect();
    if reports.is_empty() {
        explain_no_displays();
        return Ok(());
    }
    for r in reports {
        let key = r.id.key().unwrap_or_else(|| "no-edid".into());
        let input = match r.current_input {
            Some(c) => inputs::label(c),
            None => "unknown".into(),
        };
        let trust = match r.trust {
            ReadTrust::Trusted => "reads ok",
            ReadTrust::NoReplies => "reads dead",
            ReadTrust::Constant { .. } => "reads lie",
            ReadTrust::Inconclusive => "reads doubtful",
        };
        println!(
            "{:<28} {:<24} input {:<26} {}",
            r.id.label_with_fallback(&r.backend_id),
            key,
            input,
            trust
        );
    }
    Ok(())
}

/// Print each display's raw capabilities string.
fn caps() -> Result<()> {
    let displays = ddc_hi::Display::enumerate();
    if displays.is_empty() {
        explain_no_displays();
        return Ok(());
    }
    for mut display in displays {
        println!("=== {} [{}] ===", display.info.id, display.info.backend);
        let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
        match vcp.capabilities_raw() {
            Ok(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
            Err(e) => println!("unavailable: {e}"),
        }
        println!();
    }
    Ok(())
}
