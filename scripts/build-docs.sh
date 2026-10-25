#!/bin/sh
# Build the mdBook docs.
#
# Used by Vercel (vercel.json), whose build image has curl/tar but no mdBook:
# the pinned release binary is fetched, then `mdbook build docs` runs.
# Local use works the same way if you do not want a global `cargo install`:
#
#   sh scripts/build-docs.sh        # → docs/book
#
# Override the version with MDBOOK_VERSION=… .
set -eu

VERSION="${MDBOOK_VERSION:-0.5.4}"
case "$(uname -m)" in
    x86_64|amd64)   TARGET=x86_64-unknown-linux-gnu ;;
    aarch64|arm64)  TARGET=aarch64-unknown-linux-musl ;;
    *) echo "build-docs: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

BIN=".cache/mdbook"
mkdir -p "$BIN"
URL="https://github.com/rust-lang/mdBook/releases/download/v${VERSION}/mdbook-v${VERSION}-${TARGET}.tar.gz"
echo "build-docs: fetching mdBook ${VERSION} (${TARGET})"
curl -fsSL "$URL" | tar -xz -C "$BIN"
exec "$BIN/mdbook" build docs
