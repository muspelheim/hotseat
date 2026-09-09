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
}

/// Render a snippet that runs `hotseat give <peer>`.
///
/// `exe` must be absolute; see the module docs for why.
pub fn give_snippet(flavour: Flavour, exe: &Path, peer: &str) -> String {
    let exe = exe.display();
    match flavour {
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
    fn every_flavour_embeds_the_absolute_path() {
        // The whole point of this module. A relative or bare command name here
        // is the silent-failure bug reintroduced.
        for flavour in [Flavour::Skhd, Flavour::AutoHotkey, Flavour::Command] {
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
