# FAQ

This page grows with the project. Open a [chore-template issue](https://github.com/cerinoligutom/PRism/issues/new/choose) to suggest additions.

## Is PRism read-only?

V1, yes. PRism observes and surfaces state; all write actions (approve, comment, merge, request changes, resolve threads) happen on GitHub via the "Open in browser" jump on every PR row. See [Architecture](Architecture) and PRD §5.8.

## Why polling and not webhooks?

V1 has no hosted backend, which a webhook callback needs. Polling with ETag / `If-Modified-Since` keeps us inside the rate-limit budget. See ADR [0004](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0004-sync-polling-with-etag.md).

## Why PATs and not OAuth?

OAuth web flow needs a public callback URL, which would require hosting. Device Flow is viable and is on the post-v1 list. See ADR [0005](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0005-pat-auth-and-keychain-storage.md).

## Where does my token live?

Exclusively in the OS keychain — macOS Keychain, Windows Credential Manager, or libsecret on Linux. Never in plaintext files, logs, or telemetry. See ADR 0005 and PRD §7.4, §8.4.

## Does PRism work with GitHub Enterprise?

Yes, via per-account host configuration. See PRD §5.1 and §8.5.

## How fresh is the data I see?

Default sync interval is 60 seconds, configurable 30 seconds to 10 minutes. The app shows a "last synced N ago" indicator at all times — if sync is failing, you'll see it. See PRD §8.3 and ADR 0004.
