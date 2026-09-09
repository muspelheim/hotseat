//! Finding and identifying the display a command should act on.
//!
//! Every command needs the same preamble: enumerate, recover identity, resolve
//! a config key. Doing it in one place keeps the key that config is written
//! under identical to the key commands look up.

use anyhow::{Result, bail};

use crate::monitor::{MonitorId, MonitorKey};
use crate::platform::{self, Recovery};
use crate::vcp::Vcp;

/// A display, identified and ready to act on.
pub struct Found {
    /// Live handle.
    pub display: ddc_hi::Display,
    /// Recovered identity.
    pub id: MonitorId,
    /// Config key, when one could be built.
    pub key: Option<MonitorKey>,
    /// Raw capabilities string, when the display answered.
    pub capabilities: Option<Vec<u8>>,
    /// Backend handle, used for labelling when identity is thin.
    pub backend_id: String,
    /// What identity recovery managed to add. Carried rather than discarded so
    /// reports can state honestly where the identity came from.
    pub recovery: Recovery,
}

impl Found {
    /// Label for output.
    pub fn label(&self) -> String {
        self.id.label_with_fallback(&self.backend_id)
    }

    /// Config key or a clear error explaining why there is none.
    pub fn require_key(&self) -> Result<&MonitorKey> {
        match &self.key {
            Some(k) => Ok(k),
            None => bail!(
                "cannot identify {} well enough to store config against it: the backend \
                 exposed no panel identity and the display returned no capabilities \
                 string. Run `hotseat probe` for the details.",
                self.label()
            ),
        }
    }
}

/// Enumerate every DDC-capable display, identified.
pub fn find_all() -> Vec<Found> {
    ddc_hi::Display::enumerate()
        .into_iter()
        .map(|mut display| {
            let backend_id = display.info.id.clone();
            // Identity before capabilities: `update_capabilities` overwrites
            // `model_name` with the capabilities string's model field, which is
            // an internal codename on some panels.
            let mut id = MonitorId::from_info(&display.info);
            let recovery = platform::recover_identity(&mut id, &display.handle);

            let capabilities = {
                let mut vcp = crate::vcp::Handle::new(&mut display.handle);
                vcp.capabilities_raw().ok()
            };
            let key = id.resolve_key(capabilities.as_deref());

            Found {
                display,
                id,
                key,
                capabilities,
                backend_id,
                recovery,
            }
        })
        .collect()
}

/// What is actually wrong when no displays enumerate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmptyReason {
    /// No I2C bus nodes exist at all.
    NoI2cNodes,
    /// Bus nodes exist but this user cannot open them.
    I2cNotReadable {
        /// How many nodes were found.
        count: usize,
        /// Name of the group owning them, when resolvable.
        group: Option<String>,
        /// Whether this process is currently in that group.
        in_group: bool,
    },
    /// The I2C layer looks fine, so the cause is elsewhere.
    LooksAccessible,
    /// Nothing platform-specific to check.
    Unknown,
}

/// Render an empty enumeration as a diagnosis, not a list of possibilities.
///
/// The earlier version printed three candidate causes and left the reader to
/// guess. In practice the cause was almost always the one thing that can be
/// checked directly: bus nodes present but unreadable because the user's group
/// membership had not taken effect in that shell. Saying so is worth far more
/// than listing it third.
pub fn no_displays_message() -> String {
    render_empty(diagnose_empty())
}

/// Render a diagnosis. Split out from [`no_displays_message`] so every branch
/// can be asserted in tests without needing the matching hardware state.
pub fn render_empty(reason: EmptyReason) -> String {
    let mut s = String::from("No DDC-capable displays found.\n\n");
    s.push_str("This is not the same as having no monitor attached.\n\n");

    match reason {
        EmptyReason::I2cNotReadable {
            count,
            group,
            in_group,
        } => {
            s.push_str(&format!(
                "CAUSE FOUND: {count} I2C bus node(s) exist under /dev/i2c-* but this\n\
                 process cannot open them.\n\n"
            ));
            match (&group, in_group) {
                (Some(g), true) => s.push_str(&format!(
                    "You are in the '{g}' group that owns them, so this shell simply\n\
                     predates the grant. Group membership applies only to sessions\n\
                     started afterwards. Open a new login session:\n\n\
                     \x20   ssh <this host>          # or log out and back in\n\
                     \x20   exec su - \"$USER\"        # same shell, fresh groups\n\n\
                     Then try again.\n"
                )),
                (Some(g), false) => s.push_str(&format!(
                    "You are NOT in the '{g}' group that owns them. Add yourself and\n\
                     start a new login session:\n\n\
                     \x20   sudo usermod -aG {g} \"$USER\"\n\n\
                     Or run hotseat's installer, which does this for you.\n"
                )),
                (None, _) => s.push_str(
                    "Could not resolve the owning group. Check `ls -l /dev/i2c-*` and\n\
                     make sure your user can read those nodes.\n",
                ),
            }
            return s;
        }
        EmptyReason::NoI2cNodes => {
            s.push_str(
                "CAUSE FOUND: no /dev/i2c-* nodes exist at all.\n\n\
                 Either the i2c-dev interface is unavailable, or this GPU exposes no\n\
                 I2C buses. Try:\n\n\
                 \x20   sudo modprobe i2c-dev\n\n\
                 Or run hotseat's installer, which sets this up permanently.\n",
            );
            return s;
        }
        EmptyReason::LooksAccessible => {
            s.push_str(
                "The I2C layer looks fine: bus nodes exist and are readable. So the\n\
                 cause is most likely one of:\n\n",
            );
        }
        EmptyReason::Unknown => s.push_str("Likely causes:\n\n"),
    }

    s.push_str(
        "  * The display is asleep or the screen is locked. DDC enumeration returns\n\
         \x20   nothing while a panel is dark, even though the OS still lists it.\n\
         \x20   Wake the display and try again.\n",
    );
    if cfg!(target_os = "macos") {
        s.push_str(
            "  * The link does not carry DDC at all. Notably, some tools cannot reach\n\
             \x20   displays behind the built-in HDMI port of M1 and entry-level M2 Macs.\n",
        );
    }
    s.push_str("  * The monitor has no DDC-capable input on this connection.\n");
    s
}

/// Inspect the platform for a concrete cause.
pub fn diagnose_empty() -> EmptyReason {
    #[cfg(target_os = "linux")]
    {
        linux_diagnosis()
    }
    #[cfg(not(target_os = "linux"))]
    {
        EmptyReason::Unknown
    }
}

/// Look up a numeric gid in /etc/group.
///
/// Parsed directly rather than through libc's getgrgid, to avoid pulling a C
/// binding in for one lookup that only ever runs on an error path.
#[cfg(target_os = "linux")]
fn group_name(gid: u32) -> Option<String> {
    let text = std::fs::read_to_string("/etc/group").ok()?;
    for line in text.lines() {
        // name:passwd:gid:members
        let mut parts = line.split(':');
        let name = parts.next()?;
        let _passwd = parts.next();
        let found: u32 = parts.next()?.parse().ok()?;
        if found == gid {
            return Some(name.to_owned());
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_diagnosis() -> EmptyReason {
    use std::os::unix::fs::MetadataExt;

    let mut nodes: Vec<std::path::PathBuf> = match std::fs::read_dir("/dev") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("i2c-"))
            })
            .collect(),
        Err(_) => return EmptyReason::Unknown,
    };
    if nodes.is_empty() {
        return EmptyReason::NoI2cNodes;
    }
    nodes.sort();

    // Attempting to open is the only honest test: mode bits and group lists can
    // both look right while the open still fails.
    let first = &nodes[0];
    match std::fs::File::open(first) {
        Ok(_) => EmptyReason::LooksAccessible,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let group = std::fs::metadata(first)
                .ok()
                .and_then(|m| group_name(m.gid()));
            let in_group = match &group {
                Some(g) => current_groups().iter().any(|c| c == g),
                None => false,
            };
            EmptyReason::I2cNotReadable {
                count: nodes.len(),
                group,
                in_group,
            }
        }
        Err(_) => EmptyReason::Unknown,
    }
}

/// Group names this process belongs to, resolved via /proc and /etc/group.
#[cfg(target_os = "linux")]
fn current_groups() -> Vec<String> {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return Vec::new();
    };
    let Some(line) = status.lines().find(|l| l.starts_with("Groups:")) else {
        return Vec::new();
    };
    line.trim_start_matches("Groups:")
        .split_whitespace()
        .filter_map(|g| g.parse::<u32>().ok())
        .filter_map(group_name)
        .collect()
}

/// Pick exactly one display, optionally filtered by a key substring.
///
/// Refuses to guess when several match: acting on the wrong monitor is worse
/// than asking.
pub fn find_one(selector: Option<&str>) -> Result<Found> {
    let all = find_all();
    if all.is_empty() {
        bail!("{}", no_displays_message());
    }

    let mut matched: Vec<Found> = match selector {
        None => all,
        Some(want) => {
            let needle = want.to_lowercase();
            all.into_iter()
                .filter(|f| {
                    f.key
                        .as_ref()
                        .is_some_and(|k| k.value.to_lowercase().contains(&needle))
                        || f.label().to_lowercase().contains(&needle)
                })
                .collect()
        }
    };

    match matched.len() {
        0 => bail!(
            "no display matches {:?}. Run `hotseat status` to see what is attached.",
            selector.unwrap_or("")
        ),
        1 => Ok(matched.remove(0)),
        n => {
            let list: Vec<String> = matched
                .iter()
                .map(|f| {
                    let key = f
                        .key
                        .as_ref()
                        .map(|k| k.value.clone())
                        .unwrap_or_else(|| "no-key".into());
                    format!("  {}  {}", key, f.label())
                })
                .collect();
            bail!(
                "{n} displays match. Narrow it with --monitor <key>:\n{}",
                list.join("\n")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_permission_problem_is_named_outright() {
        // The case that actually happened: nodes present, user in the group,
        // but the shell predated the grant. The old message listed this third
        // among "common causes" and left the reader to guess.
        let out = render_empty(EmptyReason::I2cNotReadable {
            count: 14,
            group: Some("i2c".into()),
            in_group: true,
        });
        assert!(out.contains("CAUSE FOUND"), "{out}");
        assert!(out.contains("14 I2C bus node"), "{out}");
        assert!(out.contains("predates the grant"), "{out}");
        // Must not send someone hunting for a sleeping display instead.
        assert!(!out.contains("display is asleep"), "{out}");
    }

    #[test]
    fn not_being_in_the_group_gets_a_different_fix() {
        let out = render_empty(EmptyReason::I2cNotReadable {
            count: 2,
            group: Some("i2c".into()),
            in_group: false,
        });
        assert!(out.contains("usermod -aG i2c"), "{out}");
        assert!(!out.contains("predates the grant"), "{out}");
    }

    #[test]
    fn missing_nodes_suggest_the_module() {
        let out = render_empty(EmptyReason::NoI2cNodes);
        assert!(out.contains("modprobe i2c-dev"), "{out}");
    }

    #[test]
    fn an_accessible_bus_falls_back_to_the_remaining_causes() {
        let out = render_empty(EmptyReason::LooksAccessible);
        assert!(out.contains("looks fine"), "{out}");
        assert!(out.contains("display is asleep"), "{out}");
        // Nothing was found, so it must not claim a cause.
        assert!(!out.contains("CAUSE FOUND"), "{out}");
    }

    #[test]
    fn an_unresolvable_group_still_says_something_actionable() {
        let out = render_empty(EmptyReason::I2cNotReadable {
            count: 1,
            group: None,
            in_group: false,
        });
        assert!(out.contains("ls -l /dev/i2c-*"), "{out}");
    }
}
