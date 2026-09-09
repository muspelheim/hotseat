//! On-disk configuration.
//!
//! Config records what discovery worked out, so it is not re-derived on every
//! invocation and so verified facts survive restarts. It is plain TOML in the
//! platform config directory and meant to be readable and hand-editable.
//!
//! Two design rules earn their keep here:
//!
//! * **Monitors are keyed by panel, not by port.** A backend handle changes when
//!   you move a cable, which is precisely the event hotseat exists to handle.
//!   See [`crate::monitor::MonitorId::resolve_key`].
//! * **Nothing is written unless it was verified or a human said so.** A guess
//!   that has been persisted is indistinguishable from a fact, so guesses stay
//!   out of the file.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::monitor::KeySource;

/// Current config schema version.
pub const VERSION: u32 = 1;

/// Another machine sharing a monitor with this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// Name this peer is known by.
    pub name: String,
    /// VCP `0x60` value that selects this peer's input.
    pub input: u8,
}

/// What is known about one monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// Config key, from [`crate::monitor::MonitorId::resolve_key`].
    pub key: String,
    /// Which tier produced the key. Recorded so a later run can notice it has
    /// improved — a fingerprint key superseded by real panel identity, say.
    pub key_source: KeySource,
    /// Human label, for output only.
    pub label: String,
    /// Input this machine occupies, once known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own_input: Option<u8>,
    /// Whether this machine can reclaim the monitor after handing it away.
    ///
    /// `None` until measured. Many panels accept an input change only from the
    /// machine currently being displayed, making this `false` — which is the
    /// reason the M3 mesh has to exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_pull: Option<bool>,
    /// Values confirmed to actually switch this display.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified_inputs: Vec<u8>,
    /// Other machines and the inputs they occupy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<Peer>,
}

impl MonitorConfig {
    /// A fresh entry for a newly seen monitor, with nothing assumed.
    pub fn new(key: String, key_source: KeySource, label: String) -> Self {
        Self {
            key,
            key_source,
            label,
            own_input: None,
            can_pull: None,
            verified_inputs: Vec::new(),
            peers: Vec::new(),
        }
    }

    /// Look up a peer by name, case-insensitively.
    pub fn peer(&self, name: &str) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Record a value as confirmed to work, without duplicating it.
    pub fn mark_verified(&mut self, code: u8) {
        if !self.verified_inputs.contains(&code) {
            self.verified_inputs.push(code);
            self.verified_inputs.sort_unstable();
        }
    }

    /// Describe a value by what it means on THIS monitor.
    ///
    /// The MCCS name is actively misleading on panels using vendor codes: the
    /// reference monitor selects HDMI-1 with value 5, which MCCS calls
    /// Composite-1, so hotseat cheerfully reported "this machine is on
    /// Composite-1". Naming the machine that actually lives on an input is both
    /// correct and more useful than a standards table that does not apply.
    pub fn describe_input(&self, code: u8) -> String {
        if self.own_input == Some(code) {
            return format!("{code} (this machine)");
        }
        if let Some(p) = self.peers.iter().find(|p| p.input == code) {
            return format!("{code} ({})", p.name);
        }
        crate::inputs::label(code)
    }

    /// Un-record a verified value. Returns whether anything was removed.
    ///
    /// Needed because a wrongly recorded "verified" is worse than no record at
    /// all: it presents a value that provably does nothing as the one thing
    /// hotseat is most confident about. Reached in practice when MCCS 17 was
    /// marked verified for a panel that only accepts vendor code 5 for HDMI.
    pub fn unverify(&mut self, code: u8) -> bool {
        let before = self.verified_inputs.len();
        self.verified_inputs.retain(|&c| c != code);
        self.verified_inputs.len() != before
    }

    /// Drop a peer. Returns whether anything was removed.
    ///
    /// Config would otherwise be append-only, leaving stale entries that
    /// `give` will happily resolve to a machine that no longer exists.
    pub fn forget_peer(&mut self, name: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| !p.name.eq_ignore_ascii_case(name));
        self.peers.len() != before
    }

    /// Add or update a peer.
    pub fn upsert_peer(&mut self, name: &str, input: u8) {
        match self
            .peers
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            Some(existing) => existing.input = input,
            None => self.peers.push(Peer {
                name: name.to_owned(),
                input,
            }),
        }
        self.peers.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// The whole config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Schema version, so a future format change can migrate rather than
    /// silently misread an old file.
    pub version: u32,
    /// Name this machine is known by.
    pub machine: String,
    /// Per-monitor state.
    #[serde(default, rename = "monitors")]
    pub monitors: Vec<MonitorConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            machine: default_machine_name(),
            monitors: Vec::new(),
        }
    }
}

/// This machine's default name: its hostname, trimmed of the `.local` suffix
/// macOS appends, lowercased so peer lookups are predictable.
pub fn default_machine_name() -> String {
    let raw = gethostname::gethostname().to_string_lossy().to_string();
    let trimmed = raw.strip_suffix(".local").unwrap_or(&raw);
    let cleaned = trimmed.trim();
    if cleaned.is_empty() {
        "unnamed".into()
    } else {
        cleaned.to_lowercase()
    }
}

/// Where the config file lives on this platform.
pub fn path() -> Result<PathBuf> {
    // Honour an explicit override first: this is what makes the config layer
    // testable without touching the user's real file.
    if let Some(p) = std::env::var_os("HOTSEAT_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let dirs = directories::ProjectDirs::from("", "", "hotseat")
        .context("could not determine a config directory for this platform")?;
    Ok(dirs.config_dir().join("config.toml"))
}

impl Config {
    /// Load config, returning defaults when the file does not exist yet.
    pub fn load() -> Result<Self> {
        Self::load_from(&path()?)
    }

    /// Load from an explicit path.
    pub fn load_from(file: &Path) -> Result<Self> {
        if !file.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(file)
            .with_context(|| format!("reading config at {}", file.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("parsing config at {}", file.display()))?;
        if config.version > VERSION {
            bail!(
                "config at {} is version {}, but this build understands only up to {}. \
                 Upgrade hotseat rather than letting it misread the file.",
                file.display(),
                config.version,
                VERSION
            );
        }
        Ok(config)
    }

    /// Save config to the default location.
    pub fn save(&self) -> Result<PathBuf> {
        let file = path()?;
        self.save_to(&file)?;
        Ok(file)
    }

    /// Save to an explicit path, atomically.
    ///
    /// Writes a sibling temporary file and renames it over the target, so an
    /// interrupted save cannot leave a half-written config behind.
    pub fn save_to(&self, file: &Path) -> Result<()> {
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config directory {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialising config")?;

        let tmp = file.with_extension("toml.tmp");
        {
            let mut handle =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            handle.write_all(text.as_bytes())?;
            // Flush to disk before the rename, so a crash cannot leave the
            // renamed file present but empty.
            handle.sync_all()?;
        }
        fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))?;
        Ok(())
    }

    /// Find a monitor entry by key.
    pub fn monitor(&self, key: &str) -> Option<&MonitorConfig> {
        self.monitors.iter().find(|m| m.key == key)
    }

    /// Find a monitor entry by key, mutably.
    pub fn monitor_mut(&mut self, key: &str) -> Option<&mut MonitorConfig> {
        self.monitors.iter_mut().find(|m| m.key == key)
    }

    /// Get the entry for a key, creating it if absent.
    pub fn monitor_entry(
        &mut self,
        key: &str,
        key_source: KeySource,
        label: &str,
    ) -> &mut MonitorConfig {
        if let Some(index) = self.monitors.iter().position(|m| m.key == key) {
            // Keep the label fresh; a display's reported name can improve once
            // identity recovery starts working.
            self.monitors[index].label = label.to_owned();
            return &mut self.monitors[index];
        }
        self.monitors.push(MonitorConfig::new(
            key.to_owned(),
            key_source,
            label.to_owned(),
        ));
        self.monitors.last_mut().expect("just pushed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        let mut c = Config {
            version: VERSION,
            machine: "mac-studio".into(),
            monitors: Vec::new(),
        };
        let m = c.monitor_entry("serial:1129919028", KeySource::Serial, "Odyssey G52A");
        m.own_input = Some(17);
        m.can_pull = Some(false);
        m.mark_verified(17);
        m.mark_verified(15);
        m.upsert_peer("win-desktop", 15);
        c
    }

    #[test]
    fn round_trips_through_toml() {
        let original = sample();
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn verified_inputs_are_deduplicated_and_sorted() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.mark_verified(17);
        m.mark_verified(15);
        m.mark_verified(17);
        assert_eq!(m.verified_inputs, vec![15, 17]);
    }

    #[test]
    fn inputs_are_described_by_machine_not_by_a_standards_table() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.own_input = Some(5);
        m.upsert_peer("linux-pc", 15);
        // 5 is Composite-1 in MCCS but HDMI-1 on this panel, so the table name
        // would be wrong. The machine name never is.
        assert_eq!(m.describe_input(5), "5 (this machine)");
        assert_eq!(m.describe_input(15), "15 (linux-pc)");
        // Unknown values still fall back to the table rather than nothing.
        assert!(m.describe_input(18).contains("HDMI-2"));
    }

    #[test]
    fn a_wrongly_verified_value_can_be_retracted() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.mark_verified(15);
        m.mark_verified(17);
        assert!(m.unverify(17));
        assert_eq!(m.verified_inputs, vec![15]);
        assert!(!m.unverify(99));
    }

    #[test]
    fn forgetting_a_peer_removes_it_case_insensitively() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.upsert_peer("win-desktop", 15);
        m.upsert_peer("linux-pc", 15);
        assert!(m.forget_peer("WIN-DESKTOP"));
        assert_eq!(m.peers.len(), 1);
        assert_eq!(m.peers[0].name, "linux-pc");
        // Removing something absent must report that, not silently succeed.
        assert!(!m.forget_peer("nope"));
    }

    #[test]
    fn peers_are_matched_case_insensitively() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.upsert_peer("Win-Desktop", 15);
        assert_eq!(m.peer("win-desktop").unwrap().input, 15);
        // Updating must not create a duplicate under different casing.
        m.upsert_peer("WIN-DESKTOP", 17);
        assert_eq!(m.peers.len(), 1);
        assert_eq!(m.peer("win-desktop").unwrap().input, 17);
    }

    #[test]
    fn unknown_values_stay_absent_from_the_file() {
        // Nothing assumed means nothing serialised, so a human reading the file
        // can tell "not yet measured" from "measured as false".
        let c = Config {
            version: VERSION,
            machine: "m".into(),
            monitors: vec![MonitorConfig::new(
                "k".into(),
                KeySource::Serial,
                "l".into(),
            )],
        };
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(!text.contains("own_input"));
        assert!(!text.contains("can_pull"));
        assert!(!text.contains("verified_inputs"));
        assert!(!text.contains("peers"));
    }

    #[test]
    fn can_pull_false_is_recorded_distinctly_from_unknown() {
        let mut m = MonitorConfig::new("k".into(), KeySource::Serial, "l".into());
        m.can_pull = Some(false);
        let text = toml::to_string_pretty(&m).unwrap();
        assert!(text.contains("can_pull = false"));
    }

    #[test]
    fn a_missing_file_loads_as_defaults() {
        let dir = std::env::temp_dir().join(format!("hotseat-test-{}", std::process::id()));
        let file = dir.join("absent.toml");
        let c = Config::load_from(&file).unwrap();
        assert_eq!(c.version, VERSION);
        assert!(c.monitors.is_empty());
    }

    #[test]
    fn saves_and_reloads_from_disk() {
        let dir = std::env::temp_dir().join(format!("hotseat-save-{}", std::process::id()));
        let file = dir.join("config.toml");
        let original = sample();
        original.save_to(&file).unwrap();
        let reloaded = Config::load_from(&file).unwrap();
        assert_eq!(reloaded, original);
        // The atomic write must not leave its temporary behind.
        assert!(!file.with_extension("toml.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_schema_version_is_refused_rather_than_misread() {
        let dir = std::env::temp_dir().join(format!("hotseat-ver-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        fs::write(&file, "version = 999\nmachine = \"m\"\n").unwrap();
        let err = Config::load_from(&file).unwrap_err().to_string();
        assert!(err.contains("999"), "error should name the version: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn monitor_entry_is_idempotent_and_refreshes_the_label() {
        let mut c = Config::default();
        c.monitor_entry("k", KeySource::Serial, "old name")
            .own_input = Some(1);
        c.monitor_entry("k", KeySource::Serial, "better name");
        assert_eq!(c.monitors.len(), 1);
        assert_eq!(c.monitors[0].label, "better name");
        // Refreshing the label must not discard measured facts.
        assert_eq!(c.monitors[0].own_input, Some(1));
    }

    #[test]
    fn machine_names_are_normalised() {
        let name = default_machine_name();
        assert!(!name.is_empty());
        assert!(!name.ends_with(".local"), "got {name}");
        assert_eq!(name, name.to_lowercase());
    }
}
