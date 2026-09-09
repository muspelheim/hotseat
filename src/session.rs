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

/// Guidance for an empty enumeration, which is not the same as no monitor.
pub fn no_displays_message() -> String {
    let mut s = String::from(
        "No DDC-capable displays found.\n\n\
         This is not the same as having no monitor attached. Common causes:\n\n\
         \x20 * The display is asleep or the screen is locked. DDC enumeration returns\n\
         \x20   nothing while a panel is dark, even though the OS still lists it.\n\
         \x20   Wake the display and try again.\n",
    );
    if cfg!(target_os = "linux") {
        s.push_str(
            "  * The i2c-dev module is not loaded, or /dev/i2c-* is not readable by\n\
             \x20   your user.\n",
        );
    }
    if cfg!(target_os = "macos") {
        s.push_str(
            "  * The link does not carry DDC at all. Notably, some tools cannot reach\n\
             \x20   displays behind the built-in HDMI port of M1 and entry-level M2 Macs.\n",
        );
    }
    s.push_str("  * The monitor is currently showing a different input.\n");
    s
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
