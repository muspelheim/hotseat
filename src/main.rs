//! hotseat CLI.

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};

use hotseat::config::Config;
use hotseat::inputs::{self, INPUT_SELECT};
use hotseat::monitor::MonitorId;
use hotseat::report::DisplayReport;
use hotseat::session::{self, Found};
use hotseat::switch;
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

#[derive(Args, Clone, Default)]
struct MonitorSel {
    /// Act on the display whose key or name contains this text.
    ///
    /// Only needed when more than one display is attached.
    #[arg(long, short = 'M', global = true)]
    monitor: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect attached displays without writing to them.
    Probe,
    /// One line per display.
    Status,
    /// Dump each display's raw MCCS capabilities string verbatim.
    Caps,

    /// Hand the monitor to another machine.
    Give {
        /// Peer name, as configured.
        peer: String,
        /// Resolve and print what would be written, without writing it.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Reclaim the monitor for this machine.
    ///
    /// Many panels only obey the machine they are currently displaying, so this
    /// fails by design on those; the error says what to do instead.
    Take {
        /// Resolve and print what would be written, without writing it.
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Write a raw input-source value. The escape hatch when a peer is not set up.
    SetInput {
        /// VCP 0x60 value, e.g. 15 for DisplayPort-1 or 17 for HDMI-1.
        code: u8,
        /// Proceed without confirmation.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Measure whether this machine can reclaim the monitor after giving it away.
    ///
    /// Disruptive: it hands the monitor to a peer and tries to take it back.
    MeasurePull {
        /// Peer to hand the monitor to for the test.
        peer: String,
        /// Proceed without confirmation.
        #[arg(long)]
        yes: bool,
        #[command(flatten)]
        sel: MonitorSel,
    },

    /// Inspect and edit stored configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Print a ready-to-paste hotkey snippet, with an absolute binary path.
    Hotkey {
        /// Peer the hotkey should hand the monitor to.
        peer: String,
        /// Which tool to generate for.
        #[arg(long, value_enum, default_value_t = Flavour::Auto)]
        flavour: Flavour,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config file path.
    Path,
    /// Print the config file contents.
    Show,
    /// Record which input this machine is plugged into.
    OwnInput {
        /// VCP 0x60 value.
        code: u8,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Add or update a peer machine and the input it occupies.
    Peer {
        /// Peer name.
        name: String,
        /// VCP 0x60 value that selects that peer's input.
        code: u8,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Record an input value as confirmed working.
    Verified {
        /// VCP 0x60 value.
        code: u8,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Retract a value previously recorded as verified.
    Unverify {
        /// VCP 0x60 value to un-record.
        code: u8,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Remove a peer that no longer exists.
    ForgetPeer {
        /// Peer name to drop.
        name: String,
        #[command(flatten)]
        sel: MonitorSel,
    },
    /// Record whether this machine can reclaim the monitor after giving it away.
    ///
    /// Measure it with `hotseat measure-pull`, then record the answer here.
    CanPull {
        /// true if the monitor came back on its own, false if it did not.
        ///
        /// `action = Set` is required: clap defaults a `bool` argument to
        /// `SetTrue`, which is invalid for a positional and panics when the
        /// subcommand is built.
        #[arg(action = clap::ArgAction::Set)]
        value: bool,
        #[command(flatten)]
        sel: MonitorSel,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Flavour {
    /// Pick based on the current platform.
    Auto,
    Skhd,
    Autohotkey,
    Command,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Probe => probe(),
        Command::Status => status(),
        Command::Caps => caps(),
        Command::Give { peer, dry_run, sel } => give(&peer, dry_run, sel.monitor.as_deref()),
        Command::Take { dry_run, sel } => take(dry_run, sel.monitor.as_deref()),
        Command::SetInput { code, yes, sel } => set_input(code, yes, sel.monitor.as_deref()),
        Command::MeasurePull { peer, yes, sel } => measure_pull(&peer, yes, sel.monitor.as_deref()),
        Command::Config { action } => config_cmd(action),
        Command::Hotkey { peer, flavour } => hotkey(&peer, flavour),
    }
}

// ---------------------------------------------------------------- read-only

/// Gather a full read-only report for every attached display.
fn collect() -> Vec<DisplayReport> {
    session::find_all()
        .into_iter()
        .map(|found| {
            let Found {
                mut display,
                id,
                key,
                capabilities,
                backend_id,
                recovery,
            } = found;

            let product_name = id.model_name.clone();
            let capabilities_len = match &capabilities {
                Some(bytes) => Ok(bytes.len()),
                None => Err("display did not answer".to_owned()),
            };
            let _ = display.update_capabilities();

            // Surface the capabilities model only when it disagrees with the
            // product name: on some panels it is an internal codename.
            let declared_model = display
                .info
                .model_name
                .clone()
                .filter(|m| Some(m) != product_name.as_ref());

            let stored = Config::load()
                .ok()
                .and_then(|c| key.as_ref().and_then(|k| c.monitor(&k.value).cloned()));
            let verified = stored
                .as_ref()
                .map(|m| m.verified_inputs.clone())
                .unwrap_or_default();

            let declared_inputs: Vec<u8> = inputs::declared(&display.info.mccs_database)
                .into_iter()
                .map(|(c, _)| c)
                .collect();
            let candidates = inputs::candidates(&display.info.mccs_database, &verified);

            let (samples, trust) = {
                let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
                trust::evaluate(&mut vcp)
            };
            let current_input = if trust.is_trusted() {
                let mut vcp = hotseat::vcp::Handle::new(&mut display.handle);
                vcp.get(INPUT_SELECT).ok().map(|r| r.value as u8)
            } else {
                None
            };

            DisplayReport {
                backend: display.info.backend.to_string(),
                backend_id,
                id: MonitorId {
                    model_name: product_name,
                    ..id
                },
                declared_model,
                has_edid: display.info.edid_data.is_some(),
                recovery,
                key,
                capabilities: capabilities_len,
                samples,
                trust,
                candidates,
                declared_inputs,
                current_input,
            }
        })
        .collect()
}

fn probe() -> Result<()> {
    let reports = collect();
    if reports.is_empty() {
        print!("{}", session::no_displays_message());
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
        print!("{}", session::no_displays_message());
        return Ok(());
    }
    for r in reports {
        let key = r
            .key
            .as_ref()
            .map(|k| k.value.clone())
            .unwrap_or_else(|| "no-key".into());
        let stored = r.key.as_ref().and_then(|k| {
            Config::load()
                .ok()
                .and_then(|c| c.monitor(&k.value).cloned())
        });
        let input = match (r.current_input, &stored) {
            // Prefer naming the machine on that input: the MCCS name is wrong
            // on vendor-coded panels.
            (Some(c), Some(m)) => m.describe_input(c),
            (Some(c), None) => inputs::label(c),
            (None, _) => "unknown".into(),
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

fn caps() -> Result<()> {
    let displays = session::find_all();
    if displays.is_empty() {
        print!("{}", session::no_displays_message());
        return Ok(());
    }
    for found in displays {
        println!("=== {} ===", found.label());
        match &found.capabilities {
            Some(bytes) => println!("{}", String::from_utf8_lossy(bytes)),
            None => println!("unavailable: the display did not answer"),
        }
        println!();
    }
    Ok(())
}

// ------------------------------------------------------------------- config

fn config_cmd(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", hotseat::config::path()?.display());
            Ok(())
        }
        ConfigAction::Show => {
            let file = hotseat::config::path()?;
            if !file.exists() {
                println!("No config yet at {}", file.display());
                println!("It is created the first time you record something.");
                return Ok(());
            }
            print!("{}", std::fs::read_to_string(&file)?);
            Ok(())
        }
        ConfigAction::OwnInput { code, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            entry.own_input = Some(code);
            let file = config.save()?;
            println!("{}: this machine is on input {code}", found.label());
            println!("saved to {}", file.display());
            Ok(())
        }
        ConfigAction::Peer { name, code, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            entry.upsert_peer(&name, code);
            let file = config.save()?;
            println!("{}: peer {name} is on input {code}", found.label());
            println!("saved to {}", file.display());
            Ok(())
        }
        ConfigAction::Unverify { code, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            if entry.unverify(code) {
                let file = config.save()?;
                println!(
                    "{}: {} no longer recorded as verified",
                    found.label(),
                    inputs::label(code)
                );
                println!("saved to {}", file.display());
                Ok(())
            } else {
                bail!(
                    "{} was not recorded as verified on {}",
                    inputs::label(code),
                    found.label()
                )
            }
        }
        ConfigAction::ForgetPeer { name, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            if entry.forget_peer(&name) {
                let file = config.save()?;
                println!("{}: forgot peer {name}", found.label());
                println!("saved to {}", file.display());
            } else {
                let known: Vec<&str> = entry.peers.iter().map(|p| p.name.as_str()).collect();
                bail!(
                    "no peer named {name:?} on {}. Known: {}",
                    found.label(),
                    if known.is_empty() {
                        "none".to_owned()
                    } else {
                        known.join(", ")
                    }
                );
            }
            Ok(())
        }
        ConfigAction::CanPull { value, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            entry.can_pull = Some(value);
            let file = config.save()?;
            if value {
                println!("{}: can reclaim the monitor itself", found.label());
            } else {
                println!(
                    "{}: cannot reclaim the monitor - it only obeys the machine it is\n\
                     currently displaying. `hotseat take` will now refuse and say so.",
                    found.label()
                );
            }
            println!("saved to {}", file.display());
            Ok(())
        }
        ConfigAction::Verified { code, sel } => {
            let found = session::find_one(sel.monitor.as_deref())?;
            let key = found.require_key()?.clone();
            let mut config = Config::load()?;
            let entry = config.monitor_entry(&key.value, key.source, &found.label());
            entry.mark_verified(code);
            let file = config.save()?;
            println!(
                "{}: {} recorded as confirmed working",
                found.label(),
                inputs::label(code)
            );
            println!("saved to {}", file.display());
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- switching

/// Load the stored entry for a display, or explain that nothing is configured.
fn stored_for(found: &Found) -> Result<hotseat::config::MonitorConfig> {
    let key = found.require_key()?;
    let config = Config::load()?;
    config.monitor(&key.value).cloned().with_context(|| {
        format!(
            "no configuration for {} (key {}). Set it up with:\n    \
             hotseat config own-input <code>\n    \
             hotseat config peer <name> <code>\n\
             Run `hotseat probe` to see which values are plausible.",
            found.label(),
            key.value
        )
    })
}

/// Apply a resolved plan, or describe it when `dry_run` is set.
fn apply(found: &mut Found, plan: &switch::Plan, dry_run: bool) -> Result<()> {
    println!("{}", plan.describe());
    if dry_run {
        println!(
            "dry run: would write value {} to VCP {INPUT_SELECT:#04x}; nothing was sent",
            plan.code
        );
        return Ok(());
    }
    let mut vcp = hotseat::vcp::Handle::new(&mut found.display.handle);
    switch::set_input(&mut vcp, plan.code)?;
    println!("write accepted (this is not proof the monitor obeyed)");
    Ok(())
}

fn give(peer: &str, dry_run: bool, selector: Option<&str>) -> Result<()> {
    let mut found = session::find_one(selector)?;
    let stored = stored_for(&found)?;
    let plan = switch::plan_give(&stored, peer)?;
    apply(&mut found, &plan, dry_run)
}

fn take(dry_run: bool, selector: Option<&str>) -> Result<()> {
    let mut found = session::find_one(selector)?;
    let stored = stored_for(&found)?;
    let plan = switch::plan_take(&stored)?;
    apply(&mut found, &plan, dry_run)
}

fn set_input(code: u8, yes: bool, selector: Option<&str>) -> Result<()> {
    let mut found = session::find_one(selector)?;
    if !yes {
        bail!(
            "this writes {} to {} and will change what the monitor shows.\n\
             Re-run with --yes once you are ready.",
            inputs::label(code),
            found.label()
        );
    }
    let mut vcp = hotseat::vcp::Handle::new(&mut found.display.handle);
    switch::set_input(&mut vcp, code)?;
    println!(
        "{}: wrote {} (write accepted; not proof the monitor obeyed)",
        found.label(),
        inputs::label(code)
    );
    Ok(())
}

fn measure_pull(peer: &str, yes: bool, selector: Option<&str>) -> Result<()> {
    let mut found = session::find_one(selector)?;
    let key = found.require_key()?.clone();
    let stored = stored_for(&found)?;
    let away = switch::plan_give(&stored, peer)?;
    let back = stored.own_input.with_context(|| {
        "this machine's own input is not recorded, so there would be no way back.\n\
         Set it first with: hotseat config own-input <code>"
    })?;

    if !yes {
        bail!(
            "this test is disruptive. It will:\n\
             \x20 1. hand {} to {peer} (write {})\n\
             \x20 2. wait, then try to take it back (write {})\n\
             If the panel is push-only, step 2 does nothing and you will need to\n\
             press the monitor's own input button to recover.\n\
             Re-run with --yes when ready.",
            found.label(),
            inputs::label(away.code),
            inputs::label(back)
        );
    }

    println!("handing {} to {peer}...", found.label());
    {
        let mut vcp = hotseat::vcp::Handle::new(&mut found.display.handle);
        switch::set_input(&mut vcp, away.code)?;
    }

    // Give the panel time to switch before testing the reverse direction.
    std::thread::sleep(std::time::Duration::from_secs(6));

    println!("attempting to reclaim it...");
    let reclaim = {
        let mut vcp = hotseat::vcp::Handle::new(&mut found.display.handle);
        switch::set_input(&mut vcp, back)
    };

    match reclaim {
        Ok(()) => println!("reclaim write accepted (not proof the monitor obeyed)"),
        Err(e) => println!("reclaim write failed: {e:#}"),
    }

    // Only a human can see whether the panel actually moved, and this display's
    // own read-back cannot settle it: reads are in a different encoding from
    // writes on at least one real monitor. So the command reports and asks
    // rather than inventing a verdict.
    println!();
    println!("Only you can see what the monitor actually did, so hotseat will not guess.");
    println!("Record what you saw:");
    println!();
    println!("  it left for {peer}, then came back on its own:");
    println!("      hotseat config can-pull true");
    println!("      hotseat config verified {}", away.code);
    println!("      hotseat config verified {back}");
    println!();
    println!("  it left for {peer} but did NOT come back (push-only panel):");
    println!("      hotseat config can-pull false");
    println!("      hotseat config verified {}", away.code);
    println!();
    println!("  it never left at all:");
    println!(
        "      the value {} is wrong for {peer}; try another from `hotseat probe`",
        away.code
    );
    println!();
    println!("(monitor key: {})", key.value);
    Ok(())
}

// ------------------------------------------------------------------- hotkey

fn hotkey(peer: &str, flavour: Flavour) -> Result<()> {
    use hotseat::hotkey::{Flavour as F, give_snippet};

    let exe = std::env::current_exe().context("cannot determine this binary's own path")?;
    let flavour = match flavour {
        Flavour::Auto if cfg!(target_os = "macos") => F::Skhd,
        Flavour::Auto if cfg!(target_os = "windows") => F::AutoHotkey,
        Flavour::Auto => F::Command,
        Flavour::Skhd => F::Skhd,
        Flavour::Autohotkey => F::AutoHotkey,
        Flavour::Command => F::Command,
    };
    print!("{}", give_snippet(flavour, &exe, peer));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_internally_consistent() {
        // clap validates its own argument definitions only when a subcommand is
        // actually built, so a malformed one hides until someone runs exactly
        // that path. A positional `bool` slipped through this way and panicked
        // at `hotseat config can-pull false` while `--help` looked fine.
        // `debug_assert` walks the whole tree up front.
        Cli::command().debug_assert();
    }
}
