#!/usr/bin/env bash
#
# hotseat installer. Linux and macOS, from a clone or straight from the network.
#
#   curl -fsSL https://raw.githubusercontent.com/muspelheim/hotseat/main/install.sh | bash
#
# With options, note the `-s --`:
#
#   curl -fsSL https://raw.githubusercontent.com/muspelheim/hotseat/main/install.sh | bash -s -- --dry-run
#
# Options:
#   --dry-run     print every command without running any
#   --no-build    system setup only, skip building
#   --prefix DIR  where to put the binary (default ~/.local/bin)
#   --ref REF     git ref to build (default main)
#   --help
#
# Why Linux needs setup that macOS does not: DDC/CI reaches the monitor over
# the GPU's I2C bus, exposed as /dev/i2c-*, which is root-only by default. It
# also needs pkg-config and libudev headers to build, because ddc-hi reaches
# libudev through ddc-i2c -> i2c-linux -> udev -> libudev-sys. Neither
# requirement is obvious from its failure: missing permissions look exactly
# like having no monitor attached, and the missing headers surface as a bare
# pkg-config error that never mentions udev.
#
# Everything below lives in a function and main() runs on the very last line.
# That is deliberate. Piped into a shell, a transfer truncated partway would
# otherwise execute whatever prefix arrived; this way a partial file defines
# some functions and does nothing at all.

set -euo pipefail

readonly REPO_URL="https://github.com/muspelheim/hotseat"
readonly UDEV_RULE="/etc/udev/rules.d/60-hotseat-i2c.rules"
readonly MODULES_CONF="/etc/modules-load.d/i2c-dev.conf"

DRY_RUN=0
DO_BUILD=1
PREFIX="${HOTSEAT_PREFIX:-$HOME/.local/bin}"
GIT_REF="${HOTSEAT_REF:-main}"
SRC_DIR="${HOTSEAT_SRC:-$HOME/.local/share/hotseat}"
OS=""
DISTRO=""
PKG=""
SUDO=""
IN_GROUP=1
C_STEP=''; C_OK=''; C_WARN=''; C_ERR=''; C_DIM=''; C_OFF=''

# ------------------------------------------------------------------ output

setup_colour() {
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
        C_STEP=$'\033[1;34m'; C_OK=$'\033[1;32m'; C_WARN=$'\033[1;33m'
        C_ERR=$'\033[1;31m';  C_DIM=$'\033[2m';   C_OFF=$'\033[0m'
    fi
}

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

# Report a completed action. Silent under --dry-run: run() has already said
# what would happen, and printing "ok wrote /etc/..." for a file that was never
# touched would make the dry run actively misleading.
did() { [ "$DRY_RUN" -eq 1 ] || ok "$*"; }

# Write a privileged file from the given lines.
#
# Deliberately not `run $SUDO tee FILE >/dev/null`: that redirection applies to
# the whole run() call and swallows its own dry-run narration, which once made
# both file writes vanish from --dry-run output entirely.
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
    sed -n '3,17p' "${BASH_SOURCE[0]}" | sed -E 's|^#[[:space:]]?||'
    exit 0
}

# -------------------------------------------------------------------- args

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run)  DRY_RUN=1 ;;
            --no-build) DO_BUILD=0 ;;
            --prefix)   shift; [ $# -gt 0 ] || die "--prefix needs a directory"; PREFIX="$1" ;;
            --ref)      shift; [ $# -gt 0 ] || die "--ref needs a git ref"; GIT_REF="$1" ;;
            -h|--help)  usage ;;
            *)          die "unknown option: $1 (try --help)" ;;
        esac
        shift
    done
}

# ---------------------------------------------------------------- platform

detect_platform() {
    step "Detecting system"

    if [ "$(id -u)" -eq 0 ]; then
        die "do not run this as root. It calls sudo only where needed, and as
       root the build artefacts and config would land in root's home."
    fi

    case "$(uname -s)" in
        Linux)  OS="linux" ;;
        Darwin) OS="macos" ;;
        *)      die "unsupported OS: $(uname -s). Linux and macOS only." ;;
    esac

    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    elif [ "$OS" = "linux" ]; then
        warn "sudo not found; privileged steps cannot run"
    fi

    if [ "$OS" = "linux" ]; then
        if [ -r /etc/os-release ]; then
            # shellcheck disable=SC1091
            . /etc/os-release
            DISTRO="${ID:-unknown}"
        fi
        for candidate in apt-get dnf pacman zypper apk; do
            if command -v "$candidate" >/dev/null 2>&1; then PKG="$candidate"; break; fi
        done
        ok "${DISTRO:-linux} on $(uname -m), package manager: ${PKG:-none found}"
    else
        ok "macOS $(sw_vers -productVersion 2>/dev/null || echo '?') on $(uname -m)"
    fi
}

# ---------------------------------------------------------------- packages

install_packages() {
    if [ "$OS" != "linux" ]; then
        step "System packages"
        skip "macOS needs none: DDC works without privileged setup"
        return 0
    fi

    step "Installing packages"
    local need=""

    # Diagnosis tool, genuinely useful when DDC misbehaves.
    command -v i2cdetect >/dev/null 2>&1 || need="$need i2c-tools"

    if [ "$DO_BUILD" -eq 1 ]; then
        # Rust cannot link without a C compiler. Otherwise the failure appears
        # as "linker cc not found" from inside an unrelated build script.
        if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
            case "$PKG" in
                apt-get) need="$need build-essential" ;;
                dnf)     need="$need gcc" ;;
                pacman)  need="$need base-devel" ;;
                zypper)  need="$need gcc" ;;
                apk)     need="$need build-base" ;;
            esac
        fi
        command -v git >/dev/null 2>&1 || need="$need git"
        command -v curl >/dev/null 2>&1 || need="$need curl"
        command -v pkg-config >/dev/null 2>&1 || need="$need pkg-config"
        if ! pkg-config --exists libudev 2>/dev/null; then
            case "$PKG" in
                apt-get) need="$need libudev-dev" ;;
                dnf)     need="$need systemd-devel" ;;
                pacman)  need="$need systemd-libs" ;;
                zypper)  need="$need systemd-devel" ;;
                apk)     need="$need eudev-dev" ;;
            esac
        fi
    fi

    if [ -z "$need" ]; then
        skip "all present"
        return 0
    fi
    ok "need:$need"

    # shellcheck disable=SC2086  # $need is an intentional word-split list
    case "$PKG" in
        apt-get) run $SUDO apt-get update -qq && run $SUDO apt-get install -y $need ;;
        dnf)     run $SUDO dnf install -y $need ;;
        pacman)  run $SUDO pacman -S --needed --noconfirm $need ;;
        zypper)  run $SUDO zypper install -y $need ;;
        apk)     run $SUDO apk add $need ;;
        *)       warn "install these with your package manager, then re-run:$need" ;;
    esac
    did "installed"
}

# ------------------------------------------------------------ i2c on Linux

setup_i2c() {
    [ "$OS" = "linux" ] || return 0

    step "Making the i2c-dev interface available"
    shopt -s nullglob
    local existing=(/dev/i2c-*)
    shopt -u nullglob
    if [ "${#existing[@]}" -gt 0 ] || [ -d /sys/module/i2c_dev ]; then
        skip "already available (${#existing[@]} node(s))"
    elif [ ! -d /sys/module ]; then
        warn "no /sys/module -- this looks like a container, so modules cannot be loaded"
    elif run $SUDO modprobe i2c-dev; then
        did "loaded"
    else
        warn "modprobe i2c-dev failed; a custom or minimal kernel may not have it"
    fi

    step "Persisting the module across reboots"
    if [ -f "$MODULES_CONF" ] && grep -qx 'i2c-dev' "$MODULES_CONF" 2>/dev/null; then
        skip "$MODULES_CONF already lists it"
    else
        run $SUDO install -d /etc/modules-load.d
        write_file "$MODULES_CONF" 'i2c-dev'
        did "wrote $MODULES_CONF"
    fi

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

    # The udev rule is what actually makes the nodes group-accessible. Arch
    # ships one; most distros do not.
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
}

# -------------------------------------------------------------------- rust

ensure_rust() {
    [ "$DO_BUILD" -eq 1 ] || return 0

    step "Checking for a Rust toolchain"
    # Rust is commonly installed somewhere a non-login shell does not have on
    # PATH. Two cases seen in practice: rustup's own ~/.cargo/bin, and
    # Homebrew's rustup formula, which is keg-only and therefore never
    # symlinked into the prefix. Missing these would install a second,
    # redundant toolchain.
    local candidates=(
        "$HOME/.cargo/bin"
        /opt/homebrew/opt/rustup/bin
        /usr/local/opt/rustup/bin
    )
    for dir in "${candidates[@]}"; do
        if [ -x "$dir/cargo" ]; then
            PATH="$dir:$PATH"
            break
        fi
    done

    if command -v cargo >/dev/null 2>&1; then
        ok "cargo $(cargo --version 2>/dev/null | awk '{print $2}')"
        return 0
    fi

    if [ -n "${HOTSEAT_NO_RUST:-}" ]; then
        die "cargo not found and HOTSEAT_NO_RUST is set. Install Rust and re-run."
    fi

    warn "cargo not found; installing Rust via rustup (user-local, no sudo)"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  %swould run:%s rustup-init -y --no-modify-path --profile minimal\n' \
            "$C_DIM" "$C_OFF"
        return 0
    fi

    # Download to a file, then run it, rather than piping the network straight
    # into a shell. A truncated transfer then fails as a syntax error on a whole
    # file instead of half-executing.
    local tmp
    tmp="$(mktemp)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$tmp"
    sh "$tmp" -y --no-modify-path --profile minimal >/dev/null
    rm -f "$tmp"
    PATH="$HOME/.cargo/bin:$PATH"
    command -v cargo >/dev/null 2>&1 || die "rustup ran but cargo is still not on PATH"
    did "installed cargo $(cargo --version 2>/dev/null | awk '{print $2}')"
}

# ------------------------------------------------------------------ source

# Find the source tree, cloning it when this script was piped from the network
# rather than run from inside a checkout.
fetch_source() {
    [ "$DO_BUILD" -eq 1 ] || return 0

    step "Locating the source"
    if [ -f Cargo.toml ] && grep -q '^name = "hotseat"' Cargo.toml 2>/dev/null; then
        ok "using the checkout in $PWD"
        return 0
    fi

    command -v git >/dev/null 2>&1 || die "git is required to fetch the source"

    if [ -d "$SRC_DIR/.git" ]; then
        run git -C "$SRC_DIR" fetch --quiet --tags origin
        run git -C "$SRC_DIR" checkout --quiet --force "$GIT_REF"
        # A branch needs fast-forwarding to the remote; a tag has no upstream,
        # so a failure here is expected and not an error.
        run git -C "$SRC_DIR" reset --hard --quiet "origin/$GIT_REF" 2>/dev/null || true
        did "updated $SRC_DIR to $GIT_REF"
    else
        run git clone --quiet --branch "$GIT_REF" "$REPO_URL" "$SRC_DIR"
        did "cloned to $SRC_DIR ($GIT_REF)"
    fi
    [ "$DRY_RUN" -eq 1 ] || cd "$SRC_DIR"
}

# ------------------------------------------------------------------- build

build_and_install() {
    [ "$DO_BUILD" -eq 1 ] || return 0

    step "Building hotseat"
    if [ "$DRY_RUN" -eq 0 ]; then
        if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
            die "no C linker found. Rust needs one, and the failure otherwise
       surfaces as 'linker cc not found' from inside a build script."
        fi
        if [ "$OS" = "linux" ] && ! pkg-config --exists libudev 2>/dev/null; then
            die "libudev development files are missing. ddc-hi needs them via
       ddc-i2c -> i2c-linux -> udev -> libudev-sys, and without them cargo
       fails with a bare pkg-config error that never mentions udev."
        fi
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
}

# ------------------------------------------------------------------ verify

verify() {
    step "Verifying"
    if [ "$DRY_RUN" -eq 1 ]; then
        skip "dry run: nothing was changed, so nothing to verify"
        return 0
    fi

    if [ "$DO_BUILD" -eq 1 ] && [ -x "$PREFIX/hotseat" ]; then
        ok "$("$PREFIX/hotseat" --version)"
    fi

    if [ "$OS" = "linux" ]; then
        shopt -s nullglob
        local nodes=(/dev/i2c-*)
        shopt -u nullglob
        if [ "${#nodes[@]}" -eq 0 ]; then
            warn "no /dev/i2c-* nodes exist. Either i2c-dev is unavailable or this
       GPU exposes no I2C buses. hotseat will find no displays until fixed."
        else
            ok "${#nodes[@]} I2C bus node(s) present"
            if [ -r "${nodes[0]}" ]; then
                ok "readable by you"
            else
                warn "present but not yet readable by you -- see the note below"
            fi
        fi
    fi
}

next_steps() {
    printf '\n'
    if [ "$OS" = "linux" ] && [ "$IN_GROUP" -eq 0 ]; then
        printf '%sOne more step:%s group membership applies only to sessions started\n' "$C_WARN" "$C_OFF"
        printf 'after it was granted, so this shell cannot see it yet. Do one of:\n\n'
        # newgrp and sg are not on every distro: Ubuntu 26.04 ships neither, and
        # telling someone to run a command that does not exist is worse than
        # saying nothing. Only offer what is actually present.
        if command -v newgrp >/dev/null 2>&1; then
            printf '    newgrp i2c            # same shell, new group\n'
        fi
        if command -v su >/dev/null 2>&1; then
            # shellcheck disable=SC2016  # literal text to copy, not to expand
            printf '    exec su - "$USER"     # same shell, fresh group list\n'
        fi
        printf '    log out and back in   # always works\n\n'
    fi
    printf 'Then:\n\n'
    printf '    hotseat probe                       # what is attached, and what to believe\n'
    printf '    hotseat config own-input <code>     # which input this machine is on\n'
    printf '    hotseat config peer <name> <code>   # where the other machine is\n'
    printf '    hotseat give <name>                 # hand the monitor over\n\n'
    if [ "$OS" = "linux" ]; then
        printf 'To undo the privileged parts:\n'
        printf '    sudo rm -f %s %s\n' "$UDEV_RULE" "$MODULES_CONF"
        # shellcheck disable=SC2016  # literal text to copy, not to expand
        printf '    sudo gpasswd -d "$USER" i2c\n'
    fi
}

# -------------------------------------------------------------------- main

main() {
    setup_colour
    parse_args "$@"
    detect_platform
    install_packages
    setup_i2c
    ensure_rust
    fetch_source
    build_and_install
    verify
    next_steps
}

main "$@"
