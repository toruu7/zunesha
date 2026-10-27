#!/bin/sh
# Installer PATH-integration regression test. Uses a fake prebuilt tarball so
# the real network/source-build paths are never touched.
#
# Covers the shell startup edits added to avoid the "installed but not on
# PATH" trap: bash/zsh/fish files, idempotent re-runs, --no-modify-path,
# spaces/quotes in --prefix, dotfile symlinks, and uninstall cleanup.
#
# Usage: sh scripts/test-install.sh
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
ROOT=$(mktemp -d 2>/dev/null || mktemp -d -t zou-install-test)
PASS=0
FAIL=0

cleanup() { rm -rf "$ROOT"; }
trap cleanup EXIT

ok() { PASS=$((PASS + 1)); printf 'PASS %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1" >&2; }
fail() { bad "$1"; exit 1; }

assert_file_has() { # $1=file $2=fixed string $3=label
    if [ -f "$1" ] && grep -Fq "$2" "$1"; then ok "$3"; else bad "$3 ($1 missing: $2)"; fi
}
assert_file_lacks() {
    if [ ! -e "$1" ] || ! grep -Fq "$2" "$1"; then ok "$3"; else bad "$3 ($1 still has: $2)"; fi
}
assert_count() { # $1=file $2=fixed string $3=expected $4=label
    n=$(grep -Fxc "$2" "$1" 2>/dev/null || true)
    if [ "${n:-0}" = "$3" ]; then ok "$4"; else bad "$4 (count ${n:-0}, want $3 in $1)"; fi
}
assert_exists() { if [ -x "$1" ]; then ok "$2"; else bad "$2 ($1 not executable)"; fi }

# --- fake release ----------------------------------------------------------
PKG="$ROOT/pkg"
mkdir -p "$PKG/web/out"
cat > "$PKG/zou" <<'FAKE'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
    echo "zou 9.9.9-test"
    exit 0
fi
echo "fake zou $*"
FAKE
chmod 0755 "$PKG/zou"
printf '<!doctype html><title>fake dashboard</title>\n' > "$PKG/web/out/index.html"
tar -czf "$ROOT/zou.tar.gz" -C "$PKG" .
TARBALL="file://$ROOT/zou.tar.gz"

run_install() { # $1=home, extra args...
    home=$1
    shift
    HOME="$home" SHELL="${TEST_SHELL:-/bin/bash}" sh "$REPO/install.sh" \
        --tarball "$TARBALL" --prefix "$home/.local" "$@"
}

# --- 1. bash: adds both startup files, idempotent, sources cleanly ----------
H="$ROOT/h1"
mkdir -p "$H"
run_install "$H" > "$ROOT/h1.out" 2>&1 || fail "bash install failed"
assert_exists "$H/.local/bin/zou" "bash binary installed"
assert_file_has "$H/.bashrc" "# zou: PATH entry for $H/.local/bin (managed by install.sh)" "bash .bashrc marker"
assert_file_has "$H/.profile" "# zou: PATH entry for $H/.local/bin (managed by install.sh)" "bash .profile marker"
assert_file_has "$H/.bashrc" "$H/.local/bin" "bash .bashrc path"
assert_file_has "$H/.profile" "$H/.local/bin" "bash .profile path"
assert_file_has "$ROOT/h1.out" "added PATH entry to $H/.bashrc" "bash reports .bashrc update"
assert_file_has "$ROOT/h1.out" "reload this shell" "bash reports reload"
assert_file_has "$H/.local/share/zou/install.json" '"version": "9.9.9-test"' "install record records version"
assert_file_has "$H/.local/share/zou/install.json" "\"prefix\": \"$H/.local\"" "install record records prefix"

# Sourcing both files must leave exactly one copy of the directory.
PATH_OUT=$(env -i HOME="$H" PATH=/usr/bin:/bin bash --noprofile --norc -c \
    '. "$HOME/.bashrc"; . "$HOME/.profile"; printf "%s" "$PATH"' 2>/dev/null)
COUNT=$(printf '%s' "$PATH_OUT" | tr ':' '\n' | grep -Fxc "$H/.local/bin" || true)
[ "$COUNT" = 1 ] && ok "bash startup files are idempotent" || bad "bash PATH duplicated $COUNT times"
FOUND=$(env -i HOME="$H" PATH=/usr/bin:/bin bash --noprofile --norc -c \
    '. "$HOME/.profile"; command -v zou' 2>/dev/null || true)
[ "$FOUND" = "$H/.local/bin/zou" ] && ok "bash .profile resolves zou" || bad "bash .profile did not resolve zou ($FOUND)"

run_install "$H" > "$ROOT/h1b.out" 2>&1 || fail "bash re-install failed"
assert_count "$H/.bashrc" "# zou: PATH entry for $H/.local/bin (managed by install.sh)" 1 "bash re-run does not duplicate .bashrc"
assert_count "$H/.profile" "# zou: PATH entry for $H/.local/bin (managed by install.sh)" 1 "bash re-run does not duplicate .profile"

HOME="$H" SHELL=/bin/bash sh "$REPO/install.sh" --uninstall --prefix "$H/.local" > "$ROOT/h1-un.out" 2>&1 || fail "bash uninstall failed"
[ ! -e "$H/.local/bin/zou" ] && ok "bash uninstall removes binary" || bad "bash uninstall left binary"
assert_file_lacks "$H/.bashrc" "# zou: PATH entry for $H/.local/bin" "bash uninstall cleans .bashrc"
assert_file_lacks "$H/.profile" "# zou: PATH entry for $H/.local/bin" "bash uninstall cleans .profile"

# --- 2. already on PATH: no startup edit -----------------------------------
H="$ROOT/h2"
mkdir -p "$H"
HOME="$H" SHELL=/bin/bash PATH="$H/.local/bin:$PATH" \
    sh "$REPO/install.sh" --tarball "$TARBALL" --prefix "$H/.local" > "$ROOT/h2.out" 2>&1 || fail "on-PATH install failed"
[ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ] && ok "on-PATH install creates no rc files" || bad "on-PATH install edited rc files"

# --- 3. --no-modify-path ---------------------------------------------------
H="$ROOT/h3"
mkdir -p "$H"
run_install "$H" --no-modify-path > "$ROOT/h3.out" 2>&1 || fail "no-modify-path install failed"
[ ! -e "$H/.bashrc" ] && [ ! -e "$H/.profile" ] && ok "--no-modify-path leaves rc files alone" || bad "--no-modify-path edited rc"
assert_file_has "$ROOT/h3.out" "skipping shell startup edits" "--no-modify-path reports skip"

# --- 4. zsh: both zsh files ------------------------------------------------
H="$ROOT/h4"
mkdir -p "$H"
printf 'existing-zshrc\n' > "$H/.zshrc"
TEST_SHELL=/usr/bin/zsh run_install "$H" > "$ROOT/h4.out" 2>&1 || fail "zsh install failed"
assert_file_has "$H/.zshrc" "existing-zshrc" "zsh preserves existing .zshrc"
assert_file_has "$H/.zshrc" "$H/.local/bin" "zsh .zshrc path"
assert_file_has "$H/.zprofile" "$H/.local/bin" "zsh .zprofile path"
HOME="$H" SHELL=/usr/bin/zsh sh "$REPO/install.sh" --uninstall --prefix "$H/.local" > "$ROOT/h4-un.out" 2>&1 || fail "zsh uninstall failed"
assert_file_lacks "$H/.zshrc" "# zou: PATH entry" "zsh uninstall cleans .zshrc"
assert_file_lacks "$H/.zprofile" "# zou: PATH entry" "zsh uninstall cleans .zprofile"
assert_file_has "$H/.zshrc" "existing-zshrc" "zsh uninstall preserves existing .zshrc"

# --- 4b. zsh ZDOTDIR -------------------------------------------------------
H="$ROOT/h4b"
mkdir -p "$H/zdot"
printf 'existing-zdot-zshrc\n' > "$H/zdot/.zshrc"
HOME="$H" SHELL=/usr/bin/zsh ZDOTDIR="$H/zdot" sh "$REPO/install.sh" \
    --tarball "$TARBALL" --prefix "$H/.local" > "$ROOT/h4b.out" 2>&1 || fail "zsh ZDOTDIR install failed"
assert_file_has "$H/zdot/.zshrc" "$H/.local/bin" "zsh ZDOTDIR .zshrc path"
assert_file_has "$H/zdot/.zprofile" "$H/.local/bin" "zsh ZDOTDIR .zprofile path"
[ ! -e "$H/.zshrc" ] && ok "zsh ZDOTDIR does not touch home .zshrc" || bad "zsh ZDOTDIR touched home .zshrc"
HOME="$H" SHELL=/usr/bin/zsh ZDOTDIR="$H/zdot" sh "$REPO/install.sh" \
    --uninstall --prefix "$H/.local" > "$ROOT/h4b-un.out" 2>&1 || fail "zsh ZDOTDIR uninstall failed"
assert_file_lacks "$H/zdot/.zshrc" "# zou: PATH entry" "zsh ZDOTDIR uninstall cleans .zshrc"
assert_file_has "$H/zdot/.zshrc" "existing-zdot-zshrc" "zsh ZDOTDIR uninstall preserves existing .zshrc"

# --- 5. fish: config.fish uses fish syntax ---------------------------------
H="$ROOT/h5"
mkdir -p "$H/.config/fish"
printf 'set -gx EDITOR vim\n' > "$H/.config/fish/config.fish"
TEST_SHELL=/usr/bin/fish run_install "$H" > "$ROOT/h5.out" 2>&1 || fail "fish install failed"
assert_file_has "$H/.config/fish/config.fish" "set -gx EDITOR vim" "fish preserves existing config"
assert_file_has "$H/.config/fish/config.fish" "contains '$H/.local/bin' \$PATH" "fish config guards PATH"
assert_file_has "$H/.config/fish/config.fish" "set -gx PATH '$H/.local/bin' \$PATH" "fish config prepends PATH"
HOME="$H" SHELL=/usr/bin/fish sh "$REPO/install.sh" --uninstall --prefix "$H/.local" > "$ROOT/h5-un.out" 2>&1 || fail "fish uninstall failed"
assert_file_lacks "$H/.config/fish/config.fish" "# zou: PATH entry" "fish uninstall cleans config.fish"
assert_file_has "$H/.config/fish/config.fish" "set -gx EDITOR vim" "fish uninstall preserves existing config"

# --- 5b. fish XDG_CONFIG_HOME ---------------------------------------------
H="$ROOT/h5b"
XDG="$H/xdg"
mkdir -p "$XDG/fish"
printf 'set -gx EDITOR nano\n' > "$XDG/fish/config.fish"
HOME="$H" SHELL=/usr/bin/fish XDG_CONFIG_HOME="$XDG" sh "$REPO/install.sh" \
    --tarball "$TARBALL" --prefix "$H/.local" > "$ROOT/h5b.out" 2>&1 || fail "fish XDG install failed"
assert_file_has "$XDG/fish/config.fish" "contains '$H/.local/bin' \$PATH" "fish XDG config guards PATH"
assert_file_has "$XDG/fish/config.fish" "set -gx PATH '$H/.local/bin' \$PATH" "fish XDG config prepends PATH"
[ ! -e "$H/.config/fish/config.fish" ] && ok "fish XDG does not touch home config" || bad "fish XDG touched home config"
HOME="$H" SHELL=/usr/bin/fish XDG_CONFIG_HOME="$XDG" sh "$REPO/install.sh" \
    --uninstall --prefix "$H/.local" > "$ROOT/h5b-un.out" 2>&1 || fail "fish XDG uninstall failed"
assert_file_lacks "$XDG/fish/config.fish" "# zou: PATH entry" "fish XDG uninstall cleans config"
assert_file_has "$XDG/fish/config.fish" "set -gx EDITOR nano" "fish XDG uninstall preserves existing config"

# --- 6. prefix with spaces -------------------------------------------------
H="$ROOT/h6"
mkdir -p "$H"
P="$H/my zou"
HOME="$H" SHELL=/bin/bash sh "$REPO/install.sh" --tarball "$TARBALL" --prefix "$P" > "$ROOT/h6.out" 2>&1 || fail "space-prefix install failed"
assert_exists "$P/bin/zou" "space-prefix binary installed"
FOUND=$(env -i HOME="$H" PATH=/usr/bin:/bin bash --noprofile --norc -c \
    '. "$HOME/.profile"; command -v zou' 2>/dev/null || true)
[ "$FOUND" = "$P/bin/zou" ] && ok "space-prefix PATH resolves zou" || bad "space-prefix did not resolve ($FOUND)"

# --- 7. prefix with a single quote ----------------------------------------
H="$ROOT/h7"
mkdir -p "$H"
P="$H/o'brien"
HOME="$H" SHELL=/bin/bash sh "$REPO/install.sh" --tarball "$TARBALL" --prefix "$P" > "$ROOT/h7.out" 2>&1 || fail "quote-prefix install failed"
assert_exists "$P/bin/zou" "quote-prefix binary installed"
FOUND=$(env -i HOME="$H" PATH=/usr/bin:/bin bash --noprofile --norc -c \
    '. "$HOME/.profile"; command -v zou' 2>/dev/null || true)
[ "$FOUND" = "$P/bin/zou" ] && ok "quote-prefix PATH resolves zou" || bad "quote-prefix did not resolve ($FOUND)"

# --- 8. .bash_profile is preferred over .profile ---------------------------
H="$ROOT/h8"
mkdir -p "$H"
printf 'existing-profile\n' > "$H/.profile"
printf 'existing-bash-profile\n' > "$H/.bash_profile"
run_install "$H" > "$ROOT/h8.out" 2>&1 || fail "bash_profile install failed"
assert_file_has "$H/.bash_profile" "$H/.local/bin" "login line goes to .bash_profile"
assert_file_lacks "$H/.profile" "$H/.local/bin" ".profile untouched when .bash_profile exists"
assert_file_has "$H/.bashrc" "$H/.local/bin" ".bashrc still gets the line"

# --- 9. symlinked rc stays a symlink --------------------------------------
H="$ROOT/h9"
mkdir -p "$H/dotfiles"
printf 'existing\n' > "$H/dotfiles/bashrc"
ln -s dotfiles/bashrc "$H/.bashrc"
run_install "$H" > "$ROOT/h9.out" 2>&1 || fail "symlink install failed"
[ -L "$H/.bashrc" ] && ok "symlinked .bashrc stays a symlink" || bad "symlink was replaced"
assert_file_has "$H/dotfiles/bashrc" "$H/.local/bin" "symlink target gets the path"
HOME="$H" SHELL=/bin/bash sh "$REPO/install.sh" --uninstall --prefix "$H/.local" > "$ROOT/h9-un.out" 2>&1 || fail "symlink uninstall failed"
[ -L "$H/.bashrc" ] && ok "symlink survives uninstall" || bad "symlink lost on uninstall"
assert_file_lacks "$H/dotfiles/bashrc" "# zou: PATH entry" "symlink target cleaned on uninstall"

# --- 10. install record includes the requested release tag -----------------
H="$ROOT/h10"
mkdir -p "$H"
run_install "$H" --version v9.9.9 > "$ROOT/h10.out" 2>&1 || fail "tagged install failed"
assert_file_has "$H/.local/share/zou/install.json" '"tag": "v9.9.9"' "install record records tag"
HOME="$H" SHELL=/bin/bash sh "$REPO/install.sh" --uninstall --prefix "$H/.local" > "$ROOT/h10-un.out" 2>&1 || fail "tagged uninstall failed"
[ ! -e "$H/.local/share/zou/install.json" ] && ok "uninstall removes install record" || bad "uninstall left install record"

# --- 11. help mentions the opt-out ----------------------------------------
if sh "$REPO/install.sh" --help | grep -Fq "no-modify-path"; then ok "help documents --no-modify-path"; else bad "help lacks --no-modify-path"; fi

printf '\n%s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" = 0 ] || exit 1
