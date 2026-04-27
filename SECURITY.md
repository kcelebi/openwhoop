# Security

This document is the project’s lightweight security posture: what we assume, what we hardened in code, and what you should still do before running binaries in sensitive environments.

## Threat model (practical)

| Area | Risk | Mitigation |
|------|------|------------|
| **Firmware download** | Malicious or buggy ZIP from the network writes outside the target directory or fills the disk. | Safe extraction: `ZipFile::enclosed_name()`, symlink/encrypted entry rejection, size and entry-count caps (`src/openwhoop/src/api.rs`). |
| **HTTP client** | Open redirects or accidental cleartext if misconfigured. | `reqwest` client uses HTTPS-only and a limited redirect count for WHOOP API calls. |
| **Credentials** | `WHOOP_EMAIL` / `WHOOP_PASSWORD` leaked via shell history, `.env` commits, or process listing. | Never commit `.env`; prefer a secrets manager or one-off env vars; rotate WHOOP password if exposed. |
| **Local database** | `DATABASE_URL` / `sync` remote points at shared or internet-facing Postgres. | Use least-privilege DB users, TLS for Postgres, firewall rules. |
| **BLE** | Proximity attacks, rogue peripherals, or OS Bluetooth bugs. | Pair only with your device; keep OS and Bluetooth stack updated. |
| **Dependencies** | Vulnerable or malicious crates from crates.io. | Run `cargo audit` (and optionally `cargo deny`) on a schedule; pin `Cargo.lock` in version control. |

## WHOOP API and firmware ZIP (research notes)

- **Public developer API** ([developer.whoop.com](https://developer.whoop.com/api/)) documents OAuth 2.0 and user metrics (recovery, sleep, workouts, etc.). It does **not** document mobile-only **auth-service** sign-in or **firmware-service** endpoints.
- This project calls **`https://api.prod.whoop.com`** — the same production host used by WHOOP’s ecosystem — on paths consistent with the **mobile app** surface (e.g. `auth-service/v2/whoop/sign-in`, `firmware-service/v4/firmware/version`). Those endpoints are **undocumented for third-party use**; behavior can change without notice, and use may violate WHOOP’s terms of service. **You are trusting WHOOP’s servers** whenever you run `download-firmware`.
- The firmware response includes **base64-encoded ZIP** data. Until extracted, treat it as **untrusted binary**: we constrain where and how much is written to disk; you should still verify checksums or official release notes if WHOOP publishes them, and only flash hardware using workflows you understand.

## Hardening checklist before you run

1. `cargo audit` (install: `cargo install cargo-audit`).
2. Prefer a **dedicated output directory** for `download-firmware` (e.g. `./firmware` on a non-sensitive volume).
3. Do **not** run as root; limit filesystem permissions on the output dir.
4. Review `git diff` after upgrades; supply-chain attacks often land in small dependency bumps.

## Reporting issues

If you find a security bug in **this repository’s code**, please open a private disclosure with the maintainer (e.g. GitHub security advisory or email from the repo’s commit history) rather than a public issue, if possible.
