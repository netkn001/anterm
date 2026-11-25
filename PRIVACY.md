# Privacy Policy

_Last updated: 2026-11-25_

This policy describes how the **anterm** desktop application (`anterm-app`), its headless session daemon (`anterm-server`), and its auto-updater helper (`anterm-updater`) handle your data.

## Summary

- **No telemetry, analytics, crash uploaders, or tracking SDKs.**
- **No accounts or cloud sync servers.**
- **Your terminal content, command history, files, and credentials stay on your own machines.**

## Local Data Stored on Your Device

Depending on the features you use, anterm stores local files inside your configuration directory (`~/.config/anterm/` on macOS and Linux, `%APPDATA%\anterm\` on Windows, or the path passed via `--config-dir` / `$ANTERM_CONFIG_DIR`):

- **Configuration & keybindings** (`config.json`) — UI preferences, themes, fonts, and shortcuts.
- **Saved SSH profiles** (`servers.json`) — hostnames, ports, usernames, and non-secret connection metadata. Kept separate from `config.json` so you can sync dotfiles without sharing host lists.
- **Credentials & secrets** — SSH passwords and key passphrases are stored in your operating system's native keychain (macOS Keychain, Windows Credential Manager, or Linux Secret Service), never in plaintext inside `config.json`.
- **Session state, scrollback & command history** — persisted locally so terminal layouts and screens survive restarts.
- **Local logs & crash diagnostics** — written only to local log files for troubleshooting; never automatically transmitted anywhere.

## Network Activity

anterm only makes network connections in the following user-controlled cases:

1. **SSH, SFTP, port forwarding, and remote `anterm-server` sessions** — connects directly to hosts you configure or dial, using your configured SSH/jump-host settings.
2. **Optional update checks** — when `check_for_updates` is enabled, anterm queries the GitHub Releases API for `netkn001/anterm` (and downloads release packages if `auto_download_updates` is enabled or you explicitly start an update). You can disable update checks in **Settings → General / About** or by setting `"check_for_updates": false` in `config.json`. Optional HTTP/SOCKS proxying is configurable via `http_proxy`.
3. **Optional remote `anterm-server` installation** — when connecting to a remote host without `anterm-server`, you can opt in to download/install the matching release binary for that host.

## Third-Party Services

anterm embeds no third-party analytics, advertising, or telemetry SDKs. External CLI tools or coding agents you run inside terminal panes (such as `claude`, `codex`, `git`, `ssh`, or language servers) operate under their own privacy policies.

## Changes & Contact

Updates to this policy are tracked directly in this file in git history. If you have questions or concerns, please open an issue at <https://github.com/netkn001/anterm/issues>.
