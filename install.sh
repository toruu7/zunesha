#!/bin/sh
# install.sh — install `zou`, the local-first AI coding orchestrator.
#
# The classic one-liner:
#
#   curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh | sh
#
# Every URL is a flag, so a mirror, a fork, or a local checkout works the
# same — pass flags after `sh -s --`:
#
#   curl -fsSL <URL> | sh -s -- --repo https://github.com/you/zunesha
#   curl -fsSL <URL> | sh -s -- --tarball file:///tmp/rel/zou-linux-amd64.tar.gz
#
# Install flow:
#   1. Try the prebuilt binary for your OS/arch from --release-base.
#      A tarball may also contain `web/out` (the dashboard), which is
#      installed alongside the binary.
#   2. On any miss (no base configured, 404, wrong platform), build from
#      source: git clone --repo + cargo build. The dashboard is built too
#      when node/npm are available (skip with --no-dashboard).
#
# The binary lands in <prefix>/bin/zou and the dashboard in
# <prefix>/web/out — the daemon auto-discovers `web/out` by walking up from
# the executable, so no environment variables are needed.
#
# If <prefix>/bin is not on PATH, the installer adds one idempotent line to
# the startup file(s) for your shell (bash, zsh, fish, or ~/.profile) and
# prints the reload command. Use --no-modify-path to leave startup files
# untouched.
set -eu

# --- defaults (override with flags) -----------------------------------------
PREFIX="${HOME}/.local"
REPO="https://github.com/toruu7/zunesha"       # --repo
RELEASE_BASE="https://github.com/toruu7/zunesha/releases/latest/download"  # --release-base
TAG=""                                     # --version (optional path segment)
TARBALL=""                                 # --tarball (full URL override)
PROFILE="release"                          # --profile
DASHBOARD=1                                # --no-dashboard to disable
MODIFY_PATH=1                              # --no-modify-path to disable
UNINSTALL=0

die()  { printf 'install.sh: error: %s\n' "$1" >&2; exit 1; }
note() { printf '==> %s\n' "$1"; }
warn() { printf 'install.sh: warning: %s\n' "$1" >&2; }

usage() {
    cat <<'EOF'
install.sh — install the `zou` orchestrator

Usage: install.sh [flags]

  --prefix DIR        Install root (default: ~/.local). Binary goes to
                      DIR/bin/zou, dashboard to DIR/web/out.
  --repo URL          Git repository used for source builds.
  --release-base URL  Base URL for prebuilt tarballs; the script appends
                      [/VERSION/]zou-<os>-<arch>.tar.gz. For GitHub releases:
                      latest:  .../releases/latest/download
                      pinned:  .../releases/download  +  --version v1.2.3
  --version TAG       Optional release tag (inserted as a path segment).
  --tarball URL       Full URL of the prebuilt tarball (overrides the above).
  --source            Skip the prebuilt and build from source (--repo).
  --profile NAME      cargo profile for source builds (default: release;
                      use dev for a much faster local build).
  --no-dashboard      Do not build/serve the web dashboard.
  --no-modify-path    Do not edit shell startup files; print the PATH line.
  --uninstall         Remove the binary, dashboard, and managed PATH entry.
  --help              This text.

Examples:
  curl -fsSL https://raw.githubusercontent.com/toruu7/zunesha/master/install.sh | sh
  sh install.sh --source --repo https://github.com/you/zunesha
  sh install.sh --repo ../zunesha --source --profile dev --no-dashboard
EOF
}

# --- flag parsing (--flag value and --flag=value both work) ----------------
need_value() { [ "$2" -ge 2 ] || die "$1 needs a value (see --help)"; }
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)          need_value "$1" "$#"; PREFIX=$2; shift 2 ;;
        --prefix=*)        PREFIX=${1#*=}; shift ;;
        --repo)            need_value "$1" "$#"; REPO=$2; shift 2 ;;
        --repo=*)          REPO=${1#*=}; shift ;;
        --release-base)    need_value "$1" "$#"; RELEASE_BASE=$2; shift 2 ;;
        --release-base=*)  RELEASE_BASE=${1#*=}; shift ;;
        --source)          RELEASE_BASE=""; TARBALL=""; shift ;;
        --version)         need_value "$1" "$#"; TAG=$2; shift 2 ;;
        --version=*)       TAG=${1#*=}; shift ;;
        --tarball)         need_value "$1" "$#"; TARBALL=$2; shift 2 ;;
        --tarball=*)       TARBALL=${1#*=}; shift ;;
        --profile)         need_value "$1" "$#"; PROFILE=$2; shift 2 ;;
        --profile=*)       PROFILE=${1#*=}; shift ;;
        --no-dashboard)    DASHBOARD=0; shift ;;
        --no-modify-path)  MODIFY_PATH=0; shift ;;
        --uninstall)       UNINSTALL=1; shift ;;
        --help|-h)         usage; exit 0 ;;
        *)                 die "unknown flag: $1 (see --help)" ;;
    esac
done

BIN_DIR="$PREFIX/bin"
BIN="$BIN_DIR/zou"
DASH_DIR="$PREFIX/web/out"

# --- shell PATH integration ------------------------------------------------
# install.sh runs as a child process, so it cannot change the calling shell's
# PATH. When <prefix>/bin is missing, append an idempotent line to the startup
# file(s) the user's shell reads next time. The line checks PATH first, so
# sourcing both .bashrc and .profile cannot add the directory twice.
sh_quote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

SHELL_NAME=${SHELL:-}
SHELL_NAME=${SHELL_NAME##*/}
ZSH_DIR=${ZDOTDIR:-$HOME}
FISH_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"

POSIX_PATH_LINE="case \":\$PATH:\" in *:$(sh_quote "$BIN_DIR"):*) ;; *) PATH=$(sh_quote "$BIN_DIR"):\"\$PATH\" ;; esac; export PATH"
FISH_PATH_LINE="if not contains $(sh_quote "$BIN_DIR") \$PATH; set -gx PATH $(sh_quote "$BIN_DIR") \$PATH; end"
PATH_MARKER="# zou: PATH entry for $BIN_DIR (managed by install.sh)"

path_is_set() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) return 0 ;;
        *) return 1 ;;
    esac
}

PATH_ADDED=0
PATH_FOUND=0

add_path_line() { # $1 = startup file, $2 = line to add
    rc=$1
    line=$2
    [ -n "$rc" ] || return 0
    if [ -f "$rc" ] && grep -Fqx "$line" "$rc" 2>/dev/null; then
        PATH_FOUND=1
        return 0
    fi
    rcdir=$(dirname "$rc")
    if [ ! -d "$rcdir" ]; then
        mkdir -p "$rcdir" 2>/dev/null || {
            warn "could not create $rcdir; add this line manually:"
            warn "  $line"
            return 1
        }
    fi
    if { printf '\n%s\n%s\n' "$PATH_MARKER" "$line" >> "$rc"; } 2>/dev/null; then
        PATH_ADDED=1
        note "added PATH entry to $rc"
    else
        warn "could not write $rc; add this line manually:"
        warn "  $line"
    fi
}

add_path_to_shell_rcs() {
    case "$SHELL_NAME" in
        bash)
            add_path_line "$HOME/.bashrc" "$POSIX_PATH_LINE"
            # A login bash reads the first of these; interactive shells read
            # .bashrc. Add to both sides so either kind of shell gets PATH.
            if [ -f "$HOME/.bash_profile" ]; then
                add_path_line "$HOME/.bash_profile" "$POSIX_PATH_LINE"
            elif [ -f "$HOME/.bash_login" ]; then
                add_path_line "$HOME/.bash_login" "$POSIX_PATH_LINE"
            else
                add_path_line "$HOME/.profile" "$POSIX_PATH_LINE"
            fi
            ;;
        zsh)
            add_path_line "$ZSH_DIR/.zshrc" "$POSIX_PATH_LINE"
            add_path_line "$ZSH_DIR/.zprofile" "$POSIX_PATH_LINE"
            ;;
        fish)
            add_path_line "$FISH_CONFIG" "$FISH_PATH_LINE"
            ;;
        *)
            add_path_line "$HOME/.profile" "$POSIX_PATH_LINE"
            ;;
    esac
}

remove_path_line() { # $1 = startup file
    rc=$1
    [ -f "$rc" ] || return 0
    grep -Fqx "$PATH_MARKER" "$rc" 2>/dev/null || return 0
    tmp="$rc.zou.$$"
    if cp -p "$rc" "$tmp" 2>/dev/null \
        && awk -v m="$PATH_MARKER" -v p="$POSIX_PATH_LINE" -v f="$FISH_PATH_LINE" \
            '$0 != m && $0 != p && $0 != f' "$rc" > "$tmp" 2>/dev/null; then
        if [ -L "$rc" ]; then
            # Write through dotfile symlinks instead of replacing the link.
            if cat "$tmp" > "$rc" 2>/dev/null; then
                rm -f "$tmp"
                note "removed PATH entry from $rc"
            else
                rm -f "$tmp"
                warn "could not clean PATH entry from $rc"
            fi
        else
            if mv "$tmp" "$rc" 2>/dev/null; then
                note "removed PATH entry from $rc"
            else
                rm -f "$tmp"
                warn "could not clean PATH entry from $rc"
            fi
        fi
    else
        rm -f "$tmp"
        warn "could not clean PATH entry from $rc"
    fi
}

remove_path_everywhere() {
    remove_path_line "$HOME/.bashrc"
    remove_path_line "$HOME/.profile"
    remove_path_line "$HOME/.bash_profile"
    remove_path_line "$HOME/.bash_login"
    remove_path_line "$HOME/.zshrc"
    remove_path_line "$HOME/.zprofile"
    if [ "$ZSH_DIR" != "$HOME" ]; then
        remove_path_line "$ZSH_DIR/.zshrc"
        remove_path_line "$ZSH_DIR/.zprofile"
    fi
    remove_path_line "$HOME/.config/fish/config.fish"
    if [ "$FISH_CONFIG" != "$HOME/.config/fish/config.fish" ]; then
        remove_path_line "$FISH_CONFIG"
    fi
}

# --- uninstall -------------------------------------------------------------
if [ "$UNINSTALL" = 1 ]; then
    removed=0
    for path in "$BIN" "$PREFIX/web"; do
        if [ -e "$path" ]; then rm -rf "$path"; removed=1; fi
    done
    remove_path_everywhere
    [ "$removed" = 1 ] && note "removed zou from $PREFIX" || note "nothing to remove in $PREFIX"
    exit 0
fi

# --- platform --------------------------------------------------------------
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$OS" in
    linux)  OS=linux ;;
    darwin) OS=macos ;;
    *)      OS= ;;
esac
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *)            ARCH= ;;
esac

has() { command -v "$1" >/dev/null 2>&1; }

# Download a URL to a file, with curl or wget (whichever exists).
download() { # $1=url $2=dest
    if has curl; then curl -fsSL --retry 2 -o "$2" "$1"
    elif has wget; then wget -q -O "$2" "$1"
    else return 1
    fi
}

# --- workdir ---------------------------------------------------------------
TMP=$(mktemp -d 2>/dev/null || mktemp -d -t zou-install)
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

# --- step 1: prebuilt binary ----------------------------------------------
# The tarball URL is fully configurable: --tarball wins; otherwise
# RELEASE_BASE[/TAG]/zou-<os>-<arch>.tar.gz; placeholders mean "not set".
is_placeholder() {
    case "$1" in REPLACE_WITH_*|"") return 0 ;; *) return 1 ;; esac
}

try_prebuilt() {
    if [ -n "$TARBALL" ]; then
        url=$TARBALL
    elif is_placeholder "$RELEASE_BASE"; then
        return 1
    elif [ -n "$TAG" ]; then
        url="$RELEASE_BASE/$TAG/zou-$OS-$ARCH.tar.gz"
    else
        url="$RELEASE_BASE/zou-$OS-$ARCH.tar.gz"
    fi
    [ -n "$OS" ] && [ -n "$ARCH" ] || { note "unknown platform ($(uname -s)/$(uname -m)); skipping prebuilt"; return 1; }
    has curl || has wget || { note "no curl or wget; skipping prebuilt"; return 1; }

    note "trying prebuilt: $url"
    if ! download "$url" "$TMP/zou.tar.gz"; then
        note "prebuilt not available"
        return 1
    fi
    mkdir -p "$TMP/extract"
    if ! tar -xzf "$TMP/zou.tar.gz" -C "$TMP/extract" 2>/dev/null; then
        note "downloaded file is not a gzip tarball; skipping prebuilt"
        return 1
    fi
    # The binary sits at the tarball root or one directory deep.
    cand=$(find "$TMP/extract" -maxdepth 2 -type f -name zou 2>/dev/null | head -n 1)
    [ -n "$cand" ] || { note "no 'zou' binary inside the tarball; skipping prebuilt"; return 1; }

    mkdir -p "$BIN_DIR"
    cp "$cand" "$BIN" && chmod 0755 "$BIN"
    # A release tarball may ship the dashboard too.
    dash=$(find "$TMP/extract" -maxdepth 3 -type f -path '*/web/out/index.html' 2>/dev/null | head -n 1)
    if [ -n "$dash" ] && [ "$DASHBOARD" = 1 ]; then
        mkdir -p "$DASH_DIR"
        cp -R "$(dirname "$dash")/." "$DASH_DIR/" || warn "could not install the bundled dashboard"
    fi
    return 0
}

if try_prebuilt; then
    note "installed prebuilt binary"
else
    # --- step 2: build from source ----------------------------------------
    is_placeholder "$REPO" && die "no prebuilt available and --repo was not set.
  Pass --repo <git URL> (and optionally --release-base for prebuilt mirrors),
  e.g.:  curl -fsSL <URL> | sh -s -- --repo https://github.com/you/zunesha"

    has git  || die "git is required for a source build (install git and retry)"
    has cargo || die "cargo is required for a source build.
  Install Rust first:  https://sh.rustup.rs  (then re-run this script)"

    note "cloning $REPO"
    clone_args="--depth 1"
    [ -n "$TAG" ] && clone_args="$clone_args --branch $TAG"
    # shellcheck disable=SC2086
    git clone -q $clone_args "$REPO" "$TMP/src" || die "git clone failed: $REPO"

    # Dashboard (optional): the daemon serves <prefix>/web/out if present.
    if [ "$DASHBOARD" = 1 ]; then
        if has npm && has node; then
            note "building the dashboard"
            if [ -f "$TMP/src/web/package-lock.json" ]; then npm_cmd="ci"; else npm_cmd="install"; fi
            if (cd "$TMP/src/web" && npm $npm_cmd --no-audit --no-fund --loglevel=error && npm run build); then
                mkdir -p "$DASH_DIR"
                cp -R "$TMP/src/web/out/." "$DASH_DIR/" || warn "could not install the dashboard"
            else
                warn "dashboard build failed; installing the daemon only"
            fi
        else
            warn "node/npm not found; skipping the dashboard (daemon-only install)"
        fi
    fi

    note "building zou (profile: $PROFILE — release takes a few minutes)"
    (cd "$TMP/src" && cargo build --profile "$PROFILE" --locked) \
        || die "cargo build failed (see output above)"
    # `dev` profiles output to target/debug; named profiles to target/<name>.
    # Honor CARGO_TARGET_DIR if the user set one, else cargo defaults to
    # <workspace>/target.
    if [ "$PROFILE" = dev ]; then out=debug; else out=$PROFILE; fi
    target_dir="${CARGO_TARGET_DIR:-$TMP/src/target}"
    mkdir -p "$BIN_DIR"
    cp "$target_dir/$out/zou" "$BIN" && chmod 0755 "$BIN"
    note "installed from source"
fi

# --- verify + report -------------------------------------------------------
"$BIN" --version >/dev/null 2>&1 || die "installed binary failed to run ($BIN)"
note "installed $("$BIN" --version 2>/dev/null || echo zou) → $BIN"

PATH_READY=0
PATH_RELOAD=0
PATH_MANUAL=0

if path_is_set; then
    PATH_READY=1
elif [ "$MODIFY_PATH" = 1 ]; then
    PATH_ADDED=0
    PATH_FOUND=0
    add_path_to_shell_rcs
    if [ "$PATH_ADDED" = 1 ]; then
        PATH_RELOAD=1
        note "PATH updated for new shells."
    elif [ "$PATH_FOUND" = 1 ]; then
        PATH_RELOAD=1
        note "$BIN_DIR is already configured in your shell startup files."
    else
        PATH_MANUAL=1
    fi
else
    PATH_MANUAL=1
    note "skipping shell startup edits (--no-modify-path)."
fi

if [ "$PATH_MANUAL" = 1 ]; then
    warn "$BIN_DIR is not on PATH; add this line to your shell startup file:"
    if [ "$SHELL_NAME" = fish ]; then
        warn "  $FISH_PATH_LINE"
    else
        warn "  export PATH=$(sh_quote "$BIN_DIR"):\"\$PATH\""
    fi
fi

if [ "$PATH_RELOAD" = 1 ]; then
    if [ -n "${SHELL:-}" ]; then
        note "reload this shell to use zou now: exec \"$SHELL\" -l"
    else
        note "open a new terminal to use zou now."
    fi
fi

[ -d "$DASH_DIR" ] && note "dashboard: $DASH_DIR (auto-discovered by \`zou start\`)" \
                  || note "no dashboard installed (daemon-only; build it later from web/ or pass --release-base with a bundled tarball)"

printf '\nNext steps:\n'
if [ "$PATH_READY" = 0 ]; then
    if [ "$PATH_RELOAD" = 1 ]; then
        if [ -n "${SHELL:-}" ]; then
            printf '  exec "%s" -l   # reload PATH (or open a new terminal)\n' "$SHELL"
        else
            printf '  open a new terminal to reload PATH\n'
        fi
    elif [ "$SHELL_NAME" = fish ]; then
        printf '  %s   # add to your fish config, then reopen\n' "$FISH_PATH_LINE"
    else
        printf '  export PATH=%s:"$PATH"   # add to your shell rc, then reopen\n' "$(sh_quote "$BIN_DIR")"
    fi
fi
printf '  zou model          # choose the OpenCode model used by the agent\n  cd your-project && zou init\n  zou plan "your feature request"\n  zou start          # dashboard on http://127.0.0.1:57804\n'
