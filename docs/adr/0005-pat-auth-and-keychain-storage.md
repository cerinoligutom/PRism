# 0005 — Authentication: PAT-only stored in OS keychain

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#4](https://github.com/cerinoligutom/PRism/issues/4)
- **Deciders:** @cerinoligutom

## Context

PRism needs read access to a user's GitHub data across multiple accounts (PRD §5.1). The v1 application is read-only and runs locally — there is no server to handle OAuth callbacks. PATs are the only option that doesn't require hosted infrastructure. Token material must never leak to logs, crash reports, telemetry, or plaintext files (PRD §7.4, §8.4).

PRD §10 explicitly notes that classic PATs grant write scope along with read (`repo`), and that PAT expiry / rotation UX is the make-or-break operational concern: silent sync failures on expired tokens are the worst outcome.

## Decision drivers

- No hosted backend in v1 (rules out OAuth web flow without a public callback).
- Token confidentiality (PRD §7.4, §8.4).
- Multi-account support (PRD §5.1).
- GHE compatibility — per-account host configuration (PRD §5.1, §8.5).
- Clear failure UX on token expiry (PRD §5.1, §10).

## Considered options

1. **GitHub App auth** — requires a hosted backend.
2. **OAuth Device Flow** — viable without a server; complicates v1 and is deferred to post-v1 (PRD §12).
3. **PAT in plaintext config** — eliminated on security grounds.
4. **PAT in OS keychain (this ADR)** — meets confidentiality, no backend required.

## Decision

We will use **Personal Access Tokens stored exclusively in the OS keychain** via Tauri's secure storage APIs:

- **Backends:** macOS Keychain, Windows Credential Manager, libsecret on Linux.
- **PAT flavour:** fine-grained PATs are **recommended**; classic PATs are accepted. The Settings UI links to GitHub's PAT creation page with the required scopes pre-filled.
- **Required scopes (classic):** `repo` (read), `read:org`, `read:user`. Equivalent fine-grained permissions are documented in-app.
- **Multi-account:** users add multiple PATs, each labelled (e.g. "Work — Sitemate", "Personal"). Each account targets github.com or a configured GHE host.
- **Non-secret metadata** (label, host, login, scopes) lives in a small encrypted local config.
- **Expiry UX:** on any 401, surface a persistent banner ("Your token for [account] expired — re-add it") with a direct link to the Settings page. No silent failures.
- **Telemetry:** no v1 telemetry includes token material, PR content, or org/repo names (PRD §8.4).

## Consequences

### Positive

- Token material never leaves the keychain.
- Multi-account and GHE support fall out of the per-account configuration.
- Expiry failures are loud, not silent.

### Negative

- Classic `repo` scope grants read+write even though PRism never writes. Mitigation: prominent in-app messaging recommending fine-grained PATs.
- PAT rotation is manual. Mitigation: clear expiry surfacing; documentation.
- No SSO support in v1.

### Neutral / follow-ups

- Post-v1: OAuth Device Flow or GitHub App auth as alternatives (PRD §12).
- A future ADR will cover token validation cadence (on app start vs. lazy on first 401).

## References

- [Tauri secure storage](https://tauri.app/v2/reference/javascript/store/)
- [GitHub fine-grained PATs](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
- PRD §5.1, §7.4, §8.4, §10, §12
