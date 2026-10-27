#!/bin/sh
# bump-version.sh — bump the one version shared by the Rust workspace and the
# dashboard, using semantic versioning.
#
# Usage:
#   sh scripts/bump-version.sh major|minor|patch
#   sh scripts/bump-version.sh 1.2.3
#
# The source of truth is [workspace.package].version in Cargo.toml. This
# script also keeps Cargo.lock and the web package manifests in sync, then
# prints the commit/tag commands. It does not commit or push.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CARGO="$ROOT/Cargo.toml"
PACKAGE_JSON="$ROOT/web/package.json"
PACKAGE_LOCK="$ROOT/web/package-lock.json"
CARGO_LOCK="$ROOT/Cargo.lock"

die() { printf 'bump-version.sh: error: %s\n' "$1" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: sh scripts/bump-version.sh major|minor|patch
       sh scripts/bump-version.sh X.Y.Z

major  breaking changes
minor  new features / behavior
patch  fixes, docs, tests, housekeeping
EOF
}

[ "$#" -eq 1 ] || { usage; exit 2; }
case "$1" in
    -h|--help) usage; exit 0 ;;
esac

CURRENT=$(awk '
    /^\[workspace\.package\]/ { in_package = 1; next }
    /^\[/ { in_package = 0 }
    in_package && /^version = "/ {
        value = $0
        sub(/^version = "/, "", value)
        sub(/"$/, "", value)
        print value
        exit
    }
' "$CARGO")
[ -n "$CURRENT" ] || die "could not find [workspace.package].version in $CARGO"

case "$CURRENT" in
    *[!0-9.]*|.*|*.|*..*) die "current version is not X.Y.Z: $CURRENT" ;;
esac
OLD_MAJOR=${CURRENT%%.*}
REST=${CURRENT#*.}
OLD_MINOR=${REST%%.*}
OLD_PATCH=${REST#*.}

case "$1" in
    major) NEW="$((OLD_MAJOR + 1)).0.0" ;;
    minor) NEW="$OLD_MAJOR.$((OLD_MINOR + 1)).0" ;;
    patch) NEW="$OLD_MAJOR.$OLD_MINOR.$((OLD_PATCH + 1))" ;;
    *[!0-9.]*|.*|*.|*..*) die "invalid version or bump: $1 (use major|minor|patch|X.Y.Z)" ;;
    *) NEW=$1 ;;
esac

[ "$NEW" != "$CURRENT" ] || die "version is already $CURRENT"

replace_first() { # $1=file $2=old line $3=new line
    file=$1
    old=$2
    new=$3
    tmp="$file.bump.$$"
    awk -v old="$old" -v new="$new" '
        !done && $0 == old { print new; done = 1; next }
        { print }
    ' "$file" > "$tmp" || { rm -f "$tmp"; die "could not rewrite $file"; }
    mv "$tmp" "$file"
}

replace_first "$CARGO" "version = \"$CURRENT\"" "version = \"$NEW\""
replace_first "$PACKAGE_JSON" "  \"version\": \"$CURRENT\"," "  \"version\": \"$NEW\","

# package-lock.json has the version twice: the top-level metadata and the
# root package entry under `"packages": { "": { ... } }`. Target those two
# positions so a dependency that happens to be 0.1.0 is not changed.
LOCK_TMP="$PACKAGE_LOCK.bump.$$"
awk -v old="$CURRENT" -v new="$NEW" '
    !top && $0 == "  \"version\": \"" old "\"," {
        print "  \"version\": \"" new "\","
        top = 1
        next
    }
    /^    "": \{/ { in_root = 1; print; next }
    in_root && $0 == "      \"version\": \"" old "\"," {
        print "      \"version\": \"" new "\","
        in_root = 0
        next
    }
    { print }
' "$PACKAGE_LOCK" > "$LOCK_TMP" || { rm -f "$LOCK_TMP"; die "could not rewrite $PACKAGE_LOCK"; }
mv "$LOCK_TMP" "$PACKAGE_LOCK"

# Keep the workspace package versions in Cargo.lock in sync without invoking
# a build. Each workspace crate has `name` immediately followed by `version`.
CARGO_LOCK_TMP="$CARGO_LOCK.bump.$$"
awk -v old="version = \"$CURRENT\"" -v new="version = \"$NEW\"" '
    /^name = "zunesha-(api|cli|core)"$/ {
        print
        if ((getline line) > 0) {
            if (line == old) line = new
            print line
        }
        next
    }
    { print }
' "$CARGO_LOCK" > "$CARGO_LOCK_TMP" || { rm -f "$CARGO_LOCK_TMP"; die "could not rewrite $CARGO_LOCK"; }
mv "$CARGO_LOCK_TMP" "$CARGO_LOCK"

printf '✔ bumped %s → %s\n' "$CURRENT" "$NEW"
printf '\nNext:\n'
printf '  git add Cargo.toml Cargo.lock web/package.json web/package-lock.json\n'
printf '  git commit -m "release: v%s"\n' "$NEW"
printf '  git tag v%s\n' "$NEW"
printf '  git push origin master v%s\n' "$NEW"
