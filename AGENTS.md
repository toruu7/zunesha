# Repository instructions

## Versioning

Every push that changes the project must bump the shared version and publish a
matching Git tag. Use `scripts/bump-version.sh` so `Cargo.toml`, `Cargo.lock`,
`web/package.json`, and `web/package-lock.json` stay in sync:

```sh
sh scripts/bump-version.sh major|minor|patch
git add ...
git commit -m "release: vX.Y.Z"
git tag vX.Y.Z
git push origin master vX.Y.Z
```

Choose the bump from the change:

- **major** — breaking changes;
- **minor** — new features or user-visible behavior;
- **patch** — fixes, docs, tests, and housekeeping.

`zou update` and `.github/workflows/release.yml` use the `vX.Y.Z` tags, so a
version bump without a tag does not produce an update.

## Before pushing

Run the checks that CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
sh scripts/test-install.sh
```
