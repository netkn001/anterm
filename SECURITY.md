# Security Policy

## Supported Versions

Security fixes are applied to the latest release series on the `main` branch.

| Version | Supported |
| --- | :---: |
| `0.1.x` / `main` | ✓ |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, report vulnerabilities privately via **GitHub Security Advisories**:

- <https://github.com/netkn001/anterm/security/advisories/new>

Please include as much of the following as possible to help us reproduce and assess the issue quickly:

- Affected component (for example: `anterm-app`, `anterm-server`, `anterm-updater`, SSH/SFTP, keychain integration, shell integration, daemon IPC, or VT escape sequence handling).
- Operating system, architecture, and anterm version or commit hash.
- Step-by-step reproduction instructions or a minimal proof of concept.
- Expected impact and any suggested mitigation.

## Scope Highlights

We pay special attention to vulnerabilities affecting:

- **Privilege & IPC boundaries**: local Unix socket / Windows named-pipe permissions and authentication for `anterm-server`.
- **Native SSH / SFTP & credentials**: host-key verification (`verify_host_keys`), OS keychain isolation, path traversal in SFTP transfers, and port-forwarding exposure.
- **Terminal & shell integration safety**: escape-sequence injection (OSC/DCS/title/clipboard/hyperlink handling) and shell hook script injection.
- **Auto-updater integrity**: SHA-256 verification and atomic replacement paths in `anterm-updater`.

## Response Process

1. We aim to acknowledge private reports within **72 hours**.
2. We will investigate, confirm severity, and work on a fix privately.
3. Once a fix is ready, we will publish a patched release and coordinate disclosure credit if you would like to be acknowledged.
