## Summary

<!-- Briefly explain what this PR changes and why. -->

## Changes

-
-

## Verification

- [ ] `cargo check --workspace --all-targets --tests` passes with 0 warnings
- [ ] `cargo clippy --workspace --all-targets -- -A clippy::all -D clippy::unwrap_used` passes
- [ ] Headless binaries (`anterm-server`, `anterm-updater`) stay free of `gpui` / `rusqlite` / `keyring`
- [ ] Unit / integration tests added or updated (`cargo test --workspace --lib`)
- [ ] Updated `en` and `zh-CN` translations in `crates/anterm-i18n` (if UI copy changed)
- [ ] Updated docs (`README.md`, `README.zh-CN.md`, `docs/`, `CHANGELOG.md`) if user-facing behavior changed

## Screenshots / Recordings (if UI changed)

<!-- Optional: before/after screenshots or short clips for UI changes. -->
