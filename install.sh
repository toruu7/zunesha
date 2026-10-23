#!/bin/sh
# install.sh — install `zou`, the local-first AI coding orchestrator.
#
# The classic one-liner (from the repo that publishes releases):
#
#   curl -fsSL <REPLACE_WITH_INSTALL_SCRIPT_URL> | sh
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
set -eu

# --- defaults (placeholders: override with flags) --------------------------
PREFIX="${HOME}/.local"
REPO="REPLACE_WITH_YOUR_REPO_URL"          # --repo
RELEASE_BASE="REPLACE_WITH_RELEASES_BASE_URL"  # --release-base
TAG=""                                     # --version (optional path segment)
TARBALL=""                                 # --tarball (full URL override)
PROFILE="release"                          # --profile
DASHBOARD=1                                # --no-dashboard to disable
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
  --profile NAME      cargo profile for source builds (default: release;
                      use dev for a much faster local build).
  --no-dashboard      Do not build/serve the web dashboard.
  --uninstall         Remove the binary and dashboard from --prefix.
  --help              This text.

Examples:
  curl -fsSL <URL> | sh -s -- --repo https://github.com/you/zunesha
  sh install.sh --repo ../zunesha --profile dev --no-dashboard
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
        --version)         need_value "$1" "$#"; TAG=$2; shift 2 ;;
        --version=*)       TAG=${1#*=}; shift ;;
        --tarball)         need_value "$1" "$#"; TARBALL=$2; shift 2 ;;
        --tarball=*)       TARBALL=${1#*=}; shift ;;
        --profile)         need_value "$1" "$#"; PROFILE=$2; shift 2 ;;
        --profile=*)       PROFILE=${1#*=}; shift ;;
        --no-dashboard)    DASHBOARD=0; shift ;;
        --uninstall)       UNINSTALL=1; shift ;;
        --help|-h)         usage; exit 0 ;;
        *)                 die "unknown flag: $1 (see --help)" ;;
    esac
done

BIN_DIR="$PREFIX/bin"
BIN="$BIN_DIR/zou"
DASH_DIR="$PREFIX/web/out"

# --- uninstall -------------------------------------------------------------
if [ "$UNINSTALL" = 1 ]; then
    removed=0
    for path in "$BIN" "$PREFIX/web"; do
        if [ -e "$path" ]; then rm -rf "$path"; removed=1; fi
    done
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

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not on your PATH; add it for this shell:
      export PATH=\"$BIN_DIR:\$PATH\"
    (and put that line in ~/.profile or your shell rc to make it permanent)" ;;
esac

[ -d "$DASH_DIR" ] && note "dashboard: $DASH_DIR (auto-discovered by \`zou start\`)" \
                  || note "no dashboard installed (daemon-only; build it later from web/ or pass --release-base with a bundled tarball)"

printf '\nNext steps:\n  cd your-project && zou init\n  zou plan "your feature request"\n  zou start          # dashboard on http://127.0.0.1:7420\n'
