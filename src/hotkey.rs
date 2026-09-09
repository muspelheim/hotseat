//! Emitting ready-to-paste hotkey configuration.
//!
//! Until hotseat runs its own hotkey daemon, the shortest path to a working
//! keystroke is the user's existing tool — skhd, a desktop environment's
//! shortcut editor, AutoHotkey. What hotseat can do is generate the snippet
//! with the **absolute** path to its own binary.
//!
//! That is not a nicety. A hand-built version of this setup failed silently
//! because a launch agent invoked an unqualified command name, and launchd's
//! `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin` — no Homebrew, no `~/.cargo/bin`,
//! no `~/.local/bin`. The command simply did nothing, with no error anywhere.
//! Snippets are therefore always emitted from [`std::env::current_exe`].

use std::path::Path;

/// Which flavour of snippet to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// skhd, a macOS hotkey daemon.
    Skhd,
    /// AutoHotkey v2, on Windows.
    AutoHotkey,
    /// A bare command line, to paste into any shortcut editor.
    Command,
    /// triggerhappy, an evdev hotkey daemon for Linux.
    ///
    /// Works on a bare console, X11 and Wayland alike, because evdev sits below
    /// the display server entirely.
    Triggerhappy,
}

/// Render a snippet that runs `hotseat give <peer>`.
///
/// `exe` must be absolute; see the module docs for why. `user` is the account
/// the hotkey should run as, which only the Linux flavour needs.
pub fn give_snippet_as(flavour: Flavour, exe: &Path, peer: &str, user: &str) -> String {
    if flavour == Flavour::Triggerhappy {
        return triggerhappy_setup(&exe.display(), peer, user);
    }
    give_snippet(flavour, exe, peer)
}

/// The full triggerhappy setup, not just the trigger line.
///
/// Emitting only the trigger file would produce a hotkey that fails silently.
/// Debian's unit starts `thd` with `--user nobody`, so commands run as `nobody`
/// — which can read neither the invoking user's config nor `/dev/i2c-*`. And
/// `/etc/default/triggerhappy`, the obvious place to change that, is sourced
/// only by the sysvinit script and has no effect under systemd, so it is an
/// outright decoy. The drop-in below is the part that actually matters.
fn triggerhappy_setup(exe: &std::path::Display<'_>, peer: &str, user: &str) -> String {
    format!(
        "# triggerhappy setup for hotseat. Run these as root.\n\
         #\n\
         # 1. Let {user} read the input devices. Note this means anything running\n\
         #    as {user} can read every keystroke on that keyboard, which is the\n\
         #    unavoidable price of a hotkey below the display server.\n\
         sudo apt install -y triggerhappy\n\
         sudo usermod -aG input {user}\n\
         \n\
         # 2. Make thd run as {user} rather than nobody.\n\
         #    Without this the hotkey fires and the command silently does\n\
         #    nothing: nobody cannot read {user}'s config or /dev/i2c-*.\n\
         #    Editing /etc/default/triggerhappy will NOT work; systemd does not\n\
         #    source it.\n\
         sudo install -d /etc/systemd/system/triggerhappy.service.d\n\
         sudo tee /etc/systemd/system/triggerhappy.service.d/user.conf >/dev/null <<'UNIT'\n\
         [Service]\n\
         ExecStart=\n\
         ExecStart=/usr/sbin/thd --triggers /etc/triggerhappy/triggers.d/ \\\n\
         \x20   --socket /run/thd.socket --user {user} --deviceglob /dev/input/event*\n\
         UNIT\n\
         \n\
         # 3. The bindings. Field order is: keys, value, command.\n\
         #    Value 1 means key press (0 is release, 2 is autorepeat).\n\
         #    The absolute path is required: thd's environment has no useful PATH.\n\
         sudo tee /etc/triggerhappy/triggers.d/hotseat.conf >/dev/null <<'TRIGGERS'\n\
         KEY_LEFTCTRL+KEY_LEFTALT+KEY_LEFTSHIFT+KEY_M 1 {exe} give {peer}\n\
         KEY_LEFTCTRL+KEY_LEFTALT+KEY_LEFTSHIFT+KEY_N 1 {exe} take\n\
         TRIGGERS\n\
         \n\
         # 4. Apply.\n\
         sudo systemctl daemon-reload\n\
         sudo systemctl enable --now triggerhappy.service\n\
         \n\
         # Then log out and back in so the input group takes effect, and check:\n\
         #   systemctl status triggerhappy\n\
         #   sudo journalctl -u triggerhappy -f     # watch it fire\n"
    )
}

/// Render a snippet that runs `hotseat give <peer>`.
pub fn give_snippet(flavour: Flavour, exe: &Path, peer: &str) -> String {
    let exe = exe.display();
    match flavour {
        Flavour::Triggerhappy => triggerhappy_setup(&exe, peer, "YOUR_USER"),
        Flavour::Skhd => format!(
            "# ~/.config/skhd/skhdrc\n\
             # Absolute path is required: skhd runs under launchd, whose PATH\n\
             # excludes Homebrew and ~/.local/bin, so a bare `hotseat` would\n\
             # fail silently.\n\
             ctrl + alt + shift - m : {exe} give {peer}\n"
        ),
        Flavour::AutoHotkey => format!(
            "#Requires AutoHotkey v2.0\n\
             #SingleInstance Force\n\
             \n\
             ; Ctrl+Alt+Shift+M - hand the monitor to {peer}.\n\
             ^!+m:: RunWait(Format('\"{{1}}\" give {peer}', \"{exe}\"), , \"Hide\")\n"
        ),
        Flavour::Command => format!("{exe} give {peer}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn exe() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/hotseat")
    }

    #[test]
    fn the_linux_flavour_addresses_the_nobody_trap() {
        // Emitting only a trigger line would produce a hotkey that fires and
        // does nothing, because Debian's unit runs thd as `nobody`, which can
        // read neither the user's config nor /dev/i2c-*. The override is the
        // part that makes the hotkey work at all, so it must always be present.
        let out = give_snippet_as(Flavour::Triggerhappy, &exe(), "mac", "samedi");
        assert!(out.contains("--user samedi"), "{out}");
        assert!(out.contains("triggerhappy.service.d"), "{out}");
        assert!(out.contains("ExecStart="), "{out}");
        // And it must say why editing the obvious file does not work.
        assert!(out.contains("/etc/default/triggerhappy"), "{out}");
        // Both bindings, with the absolute path.
        assert!(out.contains("/opt/homebrew/bin/hotseat give mac"), "{out}");
        assert!(out.contains("/opt/homebrew/bin/hotseat take"), "{out}");
        assert!(out.contains("usermod -aG input samedi"), "{out}");
    }

    #[test]
    fn the_linux_flavour_states_the_keystroke_tradeoff() {
        // Reading evdev means seeing everything typed on that keyboard. That is
        // the price of a hotkey below the display server and must not be buried.
        let out = give_snippet_as(Flavour::Triggerhappy, &exe(), "mac", "samedi");
        assert!(out.contains("every keystroke"), "{out}");
    }

    #[test]
    fn every_flavour_embeds_the_absolute_path() {
        // The whole point of this module. A relative or bare command name here
        // is the silent-failure bug reintroduced.
        for flavour in [
            Flavour::Skhd,
            Flavour::AutoHotkey,
            Flavour::Command,
            Flavour::Triggerhappy,
        ] {
            let out = give_snippet(flavour, &exe(), "win-desktop");
            assert!(
                out.contains("/opt/homebrew/bin/hotseat"),
                "{flavour:?} lost the absolute path: {out}"
            );
            assert!(out.contains("win-desktop"), "{flavour:?} lost the peer");
        }
    }

    #[test]
    fn the_skhd_snippet_explains_why_the_path_is_absolute() {
        let out = give_snippet(Flavour::Skhd, &exe(), "pc");
        assert!(out.contains("launchd"));
        assert!(out.contains("silently"));
    }

    #[test]
    fn the_command_flavour_is_a_single_pasteable_line() {
        let out = give_snippet(Flavour::Command, &exe(), "pc");
        assert_eq!(out.lines().count(), 1);
        assert_eq!(out.trim(), "/opt/homebrew/bin/hotseat give pc");
    }

    #[test]
    fn the_autohotkey_snippet_declares_v2() {
        let out = give_snippet(Flavour::AutoHotkey, &exe(), "mac");
        assert!(out.starts_with("#Requires AutoHotkey v2.0"));
    }
}
