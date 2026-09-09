#!/usr/bin/env bash
#
# hotseat installer for Linux.
#
# Does the privileged setup DDC needs on Linux, builds the binary, and verifies
# the result. Safe to run more than once: every step checks before acting.
#
#   ./install.sh              install and build
#   ./install.sh --dry-run    print every command without running any
#   ./install.sh --no-build   set up permissions only, skip cargo
#   ./install.sh --help
#
# Why any of this is needed: on Linux, DDC/CI talks to the monitor over the
# graphics card's I2C bus, exposed as /dev/i2c-*. That needs the i2c-dev module
# loaded and the device nodes readable by your user. Without it hotseat finds no
# displays at all -- and the failure looks identical to having no monitor.

set -euo pipefail

readonly REPO_URL="https://github.com/muspelheim/hotseat"
readonly UDEV_RULE="/etc/udev/rules.d/60-hotseat-i2c.rules"
readonly MODULES_CONF="/etc/modules-load.d/i2c-dev.conf"

DRY_RUN=0
DO_BUILD=1
PREFIX="${HOME}/.local/bin"

# ------------------------------------------------------------------ output

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_STEP=$'\033[1;34m'; C_OK=$'\033[1;32m'; C_WARN=$'\033[1;33m'
    C_ERR=$'\033[1;31m';  C_DIM=$'\033[2m';   C_OFF=$'\033[0m'
else
    C_STEP=''; C_OK=''; C_WARN=''; C_ERR=''; C_DIM=''; C_OFF=''
fi

step() { printf '%s==>%s %s\n' "$C_STEP" "$C_OFF" "$*"; }
ok()   { printf '  %sok%s   %s\n' "$C_OK" "$C_OFF" "$*"; }
skip() { printf '  %sskip%s %s\n' "$C_DIM" "$C_OFF" "$*"; }
warn() { printf '  %swarn%s %s\n' "$C_WARN" "$C_OFF" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$C_ERR" "$C_OFF" "$*" >&2; exit 1; }

# Run a command, or print it under --dry-run. Every mutating action goes
# through here, so --dry-run is trustworthy rather than approximate.
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  %swould run:%s %s\n' "$C_DIM" "$C_OFF" "$*"
        return 0
    fi
    "$@"
}

# Report a completed action.
#
# Silent under --dry-run: `run` has already narrated what would happen, and
# printing "ok wrote /etc/..." for a file that was not written would make the
# dry run actively misleading.
did() { [ "$DRY_RUN" -eq 1 ] || ok "$*"; }

# Write a privileged file from the given lines.
#
# Deliberately not `run $SUDO tee FILE >/dev/null`: that redirection applies to
# the whole `run` call and swallows its own dry-run narration, so the file write
# vanished from --dry-run output entirely.
write_file() {
    local dest="$1"
    shift
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  %swould write:%s %s (%d line(s))\n' "$C_DIM" "$C_OFF" "$dest" "$#"
        return 0
    fi
    printf '%s\n' "$@" | $SUDO tee "$dest" >/dev/null
}

usage() {
    sed -n '3,20p' "$0" | sed -E 's|^#[[:space:]]?||'
    exit 0
}

# ------------------------------------------------------------------- args

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)  DRY_RUN=1 ;;
        --no-build) DO_BUILD=0 ;;
        --prefix)   shift; [ $# -gt 0 ] || die "--prefix needs a directory"; PREFIX="$1" ;;
        -h|--help)  usage ;;
        *)          die "unknown option: $1 (try --help)" ;;
    esac
    shift
done

# ------------------------------------------------------------ preflight

[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux. On macOS use \`cargo build --release\`; no privileged setup is needed there."

if [ "$(id -u)" -eq 0 ]; then
    die "do not run this as root. It calls sudo only for the steps that need it,
       and running the whole thing as root would put the build artefacts and
       config in root's home instead of yours."
fi

SUDO=""
if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
else
    warn "sudo not found; privileged steps will be skipped and printed instead"
fi

step "Detecting system"
ARCH="$(uname -m)"
DISTRO="unknown"; PKG=""
if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    DISTRO="${ID:-unknown}"
fi
for candidate in apt-get dnf pacman zypper apk; do
    if command -v "$candidate" >/dev/null 2>&1; then PKG="$candidate"; break; fi
done
ok "$DISTRO on $ARCH, package manager: ${PKG:-none found}"

# ---------------------------------------------------------------- packages

# i2c-tools is for diagnosis (`i2cdetect -l`), and is genuinely useful when DDC
# misbehaves. The rest are build requirements: ddc-hi reaches libudev through
# ddc-i2c -> i2c-linux -> udev -> libudev-sys, whose build script needs
# pkg-config and the libudev headers. Without them the build fails with a
# pkg-config error that says nothing about udev.
step "Installing packages"
NEED=""
command -v i2cdetect >/dev/null 2>&1 || NEED="$NEED i2c-tools"
if [ "$DO_BUILD" -eq 1 ]; then
    # Rust needs a C linker. A minimal install may genuinely not have one, and
    # the failure is `linker \`cc\` not found` from deep inside a build script,
    # which does not suggest installing a compiler.
    if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
        case "$PKG" in
            apt-get) NEED="$NEED build-essential" ;;
            dnf)     NEED="$NEED gcc" ;;
            pacman)  NEED="$NEED base-devel" ;;
            zypper)  NEED="$NEED gcc" ;;
            apk)     NEED="$NEED build-base" ;;
        esac
    fi
    command -v pkg-config >/dev/null 2>&1 || NEED="$NEED pkg-config"
    if ! pkg-config --exists libudev 2>/dev/null; then
        case "$PKG" in
            apt-get) NEED="$NEED libudev-dev" ;;
            dnf)     NEED="$NEED systemd-devel" ;;
            pacman)  NEED="$NEED systemd-libs" ;;
            zypper)  NEED="$NEED systemd-devel" ;;
            apk)     NEED="$NEED eudev-dev" ;;
        esac
    fi
fi

# shellcheck disable=SC2086  # NEED is an intentional word-split package list
if [ -z "$NEED" ]; then
    skip "all present"
else
    ok "need:$NEED"
    case "$PKG" in
        apt-get) run $SUDO apt-get update -qq && run $SUDO apt-get install -y $NEED ;;
        dnf)     run $SUDO dnf install -y $NEED ;;
        pacman)  run $SUDO pacman -S --needed --noconfirm $NEED ;;
        zypper)  run $SUDO zypper install -y $NEED ;;
        apk)     run $SUDO apk add $NEED ;;
        *)       warn "install these with your package manager, then re-run:$NEED" ;;
    esac
    did "installed"
fi

# ------------------------------------------------------------ i2c-dev module

step "Loading the i2c-dev kernel module"
if [ -d /sys/module/i2c_dev ]; then
    skip "already loaded"
elif [ ! -d /sys/module ]; then
    # Containers have no module machinery; say so rather than failing opaquely.
    warn "no /sys/module -- this looks like a container, so modules cannot be loaded here"
else
    # Report only what actually happened: `|| warn` swallows the failure, so
    # an unconditional success line here would contradict the warning above it.
    if run $SUDO modprobe i2c-dev; then
        did "loaded"
    else
        warn "modprobe i2c-dev failed; a custom or minimal kernel may not have it"
    fi
fi

step "Persisting the module across reboots"
if [ -f "$MODULES_CONF" ] && grep -qx 'i2c-dev' "$MODULES_CONF" 2>/dev/null; then
    skip "$MODULES_CONF already lists it"
else
    run $SUDO install -d /etc/modules-load.d
    write_file "$MODULES_CONF" 'i2c-dev'
    did "wrote $MODULES_CONF"
fi

# ------------------------------------------------------------- permissions

step "Granting your user access to /dev/i2c-*"
if getent group i2c >/dev/null 2>&1; then
    skip "group i2c exists"
else
    run $SUDO groupadd --system i2c
    did "created group i2c"
fi

if id -nG "$USER" 2>/dev/null | tr ' ' '\n' | grep -qx i2c; then
    skip "$USER is already in the i2c group"
    IN_GROUP=1
else
    run $SUDO usermod -aG i2c "$USER"
    did "added $USER to the i2c group"
    IN_GROUP=0
fi

# A udev rule is what actually makes the nodes group-accessible. Some distros
# (Arch) ship one; most do not.
step "Installing the udev rule"
if [ -f "$UDEV_RULE" ]; then
    skip "$UDEV_RULE already present"
else
    run $SUDO install -d /etc/udev/rules.d
    write_file "$UDEV_RULE" \
        '# Installed by hotseat. Makes the I2C buses used for DDC/CI readable by' \
        '# the i2c group, so monitor control does not require root.' \
        'KERNEL=="i2c-[0-9]*", GROUP="i2c", MODE="0660"'
    did "wrote $UDEV_RULE"
    if run $SUDO udevadm control --reload-rules \
        && run $SUDO udevadm trigger --subsystem-match=i2c-dev; then
        did "reloaded udev"
    else
        warn "could not reload udev; the rule applies after the next reboot"
    fi
fi

# ------------------------------------------------------------------- build

if [ "$DO_BUILD" -eq 1 ]; then
    step "Checking for a Rust toolchain"
    if command -v cargo >/dev/null 2>&1; then
        ok "cargo $(cargo --version 2>/dev/null | awk '{print $2}')"
    else
        warn "cargo not found. Install Rust, then re-run this script:"
        printf '\n      curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh\n'
        # shellcheck disable=SC2016  # literal text for the user to copy, not to expand
        printf '      . "$HOME/.cargo/env"\n\n'
        # Deliberately not piping a remote script into a shell on the user's
        # behalf. Printing it lets them read what they are running.
        die "Rust toolchain required to build hotseat"
    fi

    step "Building hotseat"
    if [ "$DRY_RUN" -eq 0 ] \
        && ! command -v cc >/dev/null 2>&1 \
        && ! command -v gcc >/dev/null 2>&1; then
        die "no C linker found. Rust needs one, and the failure otherwise
       surfaces as \`linker \\\`cc\\\` not found\` from inside a build script.
       Install your distro's C toolchain and re-run."
    fi
    if [ "$DRY_RUN" -eq 0 ] && ! pkg-config --exists libudev 2>/dev/null; then
        die "libudev development files are still missing.
       ddc-hi needs them via ddc-i2c -> i2c-linux -> udev -> libudev-sys, and
       without them cargo fails with a bare pkg-config error that never
       mentions udev. Install your distro's libudev headers and re-run."
    fi
    if [ ! -f Cargo.toml ]; then
        die "no Cargo.toml here. Run this from a clone:
       git clone $REPO_URL && cd hotseat && ./install.sh"
    fi
    run cargo build --release --quiet
    did "built target/release/hotseat"

    step "Installing to $PREFIX"
    run install -d "$PREFIX"
    run install -m 0755 target/release/hotseat "$PREFIX/hotseat"
    did "$PREFIX/hotseat"

    case ":${PATH}:" in
        *":$PREFIX:"*) : ;;
        *) warn "$PREFIX is not on your PATH; add it to your shell profile" ;;
    esac
fi

# ------------------------------------------------------------------ verify

step "Verifying"
if [ "$DRY_RUN" -eq 1 ]; then
    skip "dry run: nothing was changed, so nothing to verify"
    exit 0
fi

# Glob into an array rather than parsing `ls`: gives an exact count and lets us
# test the first node directly.
shopt -s nullglob
i2c_nodes=(/dev/i2c-*)
shopt -u nullglob
buses=${#i2c_nodes[@]}
if [ "$buses" -eq 0 ]; then
    warn "no /dev/i2c-* nodes exist. Either i2c-dev is not loaded, or this GPU
       exposes no I2C buses. hotseat will find no displays until this is fixed."
else
    ok "$buses I2C bus node(s) present"
    if [ -r "${i2c_nodes[0]}" ]; then
        ok "readable by you"
    else
        warn "present but not readable by you yet -- see the note below"
    fi
fi

printf '\n'
if [ "${IN_GROUP:-1}" -eq 0 ]; then
    printf '%sOne more step:%s group membership does not apply to shells that were\n' "$C_WARN" "$C_OFF"
    printf 'already running when it was granted. Log out and back in, or start a\n'
    printf 'fresh session, then check:\n\n'
    printf '    hotseat probe\n\n'
    printf 'To test without logging out: %snewgrp i2c%s\n\n' "$C_DIM" "$C_OFF"
else
    printf 'Next:\n\n'
    printf '    hotseat probe                       # what is attached, and what to believe\n'
    printf '    hotseat config own-input <code>     # which input this machine is on\n'
    printf '    hotseat config peer <name> <code>   # where the other machine is\n'
    printf '    hotseat give <name>                 # hand the monitor over\n\n'
fi
printf 'To undo the privileged parts:\n'
printf '    sudo rm -f %s %s\n' "$UDEV_RULE" "$MODULES_CONF"
# shellcheck disable=SC2016  # literal text for the user to copy, not to expand
printf '    sudo gpasswd -d "$USER" i2c\n'
