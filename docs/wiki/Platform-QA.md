# Platform QA

Release-gate checklist for tagging a PRism v1 build. Walk one platform start to finish before merging the release tag. Each section is independent - tick the boxes during the pass, file an issue for anything that fails.

The checklist covers macOS, Windows, and Linux. Some items only apply to one OS; those are called out per-section. CLAUDE.md is explicit that Windows in particular needs hands-on testing, so don't sign off on a release without a full Windows run.

## Before you start

- [ ] Latest `main` builds locally (`pnpm tauri:build`).
- [ ] A throwaway GitHub account with a fresh fine-grained PAT is available for the install / reauth scenarios.
- [ ] A second PAT (or the same one re-issued) is ready for the reauth flow.
- [ ] At least one repo with open PRs the test account participates in.

## macOS

Tested on the latest two major macOS versions. Apple silicon and Intel both count if you have access to both.

### Fresh install

- [ ] Bundled `.dmg` mounts and the app drags into `/Applications` without Gatekeeper blocking it (signed build only - unsigned local builds will warn, that's expected).
- [ ] First launch: the onboarding screen appears, not the dashboard.
- [ ] Adding the first account triggers the macOS Keychain prompt to allow access; granting access stores the PAT.
- [ ] First sync completes within ~30 seconds for an account with under 100 PRs. The activity feed shows the phase progression (account -> repos -> PRs -> reviews -> threads).

### Reauth flow

- [ ] Open Keychain Access and delete (or rename) the PRism entry for the account.
- [ ] Reopen PRism: the reauth dialog appears on first sync attempt.
- [ ] Pasting a fresh PAT into the dialog re-validates the account and resumes sync.

### Notifications

- [ ] Master switch under Settings -> Notifications is OFF on first launch.
- [ ] Flipping the master switch ON triggers the macOS notification permission prompt the first time a toast would fire (not at the toggle moment - defer until a real event).
- [ ] With both per-trigger toggles ON, a newly attention-needing PR fires a toast and increments the dock badge.
- [ ] Disabling the "PR newly needs your attention" toggle silences that trigger; "you were mentioned" still fires independently.
- [ ] Dock badge updates within a sync cycle when the underlying count changes. Badge clears when the count drops to zero.
- [ ] Clicking a notification focuses the window and deep-links to the relevant PR.

### Sync behaviour

- [ ] Status bar shows "Last synced N ago" at all times.
- [ ] Manual refresh (sidebar control) bumps the timestamp and runs a full pass.
- [ ] The rate-limit guard pauses sync when GitHub returns under 20% remaining; the status bar surfaces the throttle state.
- [ ] Activity feed shows each phase (account validation, repos, PRs, reviews, threads) with timing.

### Window chrome

- [ ] Traffic-light buttons (red / yellow / green) sit in the expected macOS position and behave normally.
- [ ] Full-screen via the green button works; the title bar layout adapts.
- [ ] On a HiDPI display (Retina): icons, badges, and avatars render crisply at the system scale.
- [ ] On an external display at 1x: no visible blur or sub-pixel artefacts.

### Theme

- [ ] First launch matches the system appearance (dark mode if the system is dark; light otherwise).
- [ ] Toggling the system appearance while PRism is running flips the UI live, no restart.

---

## Windows

Tested on Windows 11 (and Windows 10 if the build still targets it). Requires WebView2 runtime - note any prompt to install it.

### Fresh install

- [ ] The `.msi` (or `.exe`) installer runs without SmartScreen blocking (signed build only; unsigned will warn).
- [ ] First launch: WebView2 loads cleanly; the onboarding screen appears.
- [ ] Adding the first account stores the PAT in Windows Credential Manager - verify via `control /name Microsoft.CredentialManager` that an entry exists under PRism.
- [ ] First sync completes within ~30 seconds for a small account.

### Reauth flow

- [ ] Open Credential Manager and remove the PRism credential.
- [ ] Reopen PRism: the reauth dialog appears on first sync attempt.
- [ ] Pasting a fresh PAT re-validates and resumes sync.

### Notifications

- [ ] Master switch is OFF on first launch.
- [ ] Flipping the master switch ON, then triggering the first toast, surfaces the Windows notification permission prompt.
- [ ] Toast appears in the Action Center and respects Focus Assist (silent if Focus Assist is on; queued for review).
- [ ] Per-trigger toggles silence their own category without affecting the other.
- [ ] No dock badge equivalent on Windows in v1 - confirm the taskbar icon does not attempt to overlay a number (Architecture page documents this gap).
- [ ] Clicking a toast focuses the window and deep-links to the relevant PR.

### Sync behaviour

- [ ] Status bar shows "Last synced N ago" at all times.
- [ ] Manual refresh works from the sidebar control.
- [ ] Rate-limit guard pauses sync at 20% remaining; throttle state visible in the status bar.
- [ ] Activity feed shows phase progression.

### Window chrome

- [ ] Minimise, maximise, close behave as native Windows controls.
- [ ] Snap layouts (Win + arrow) reposition PRism cleanly.
- [ ] On a HiDPI display at 1.5x or 2x scaling: icons and text render crisply.
- [ ] On an external display at 1x while the laptop screen is 1.5x: dragging the window between displays adjusts scale without crashing or losing layout.

### Theme

- [ ] First launch matches the Windows app mode (Settings -> Personalisation -> Colours -> "Choose your default app mode").
- [ ] Flipping the system app mode while PRism is running flips the UI live.

---

## Linux

Tested on the latest Ubuntu LTS and one rolling distro (Arch / Fedora) if available. Requires WebKitGTK; the supported display servers are X11 and Wayland.

### Fresh install

- [ ] The `.AppImage` (or `.deb`) runs without complaint after `chmod +x` on AppImage, or `dpkg -i` for `.deb`.
- [ ] First launch: the onboarding screen appears.
- [ ] Adding the first account prompts for libsecret access (GNOME Keyring or KWallet, depending on desktop); granting access stores the PAT.
- [ ] First sync completes within ~30 seconds for a small account.

### Reauth flow

- [ ] Remove the PRism entry from the keyring (`seahorse` on GNOME, KWalletManager on KDE).
- [ ] Reopen PRism: the reauth dialog appears on first sync attempt.
- [ ] Pasting a fresh PAT re-validates and resumes sync.

### Notifications

- [ ] Master switch is OFF on first launch.
- [ ] Flipping the master switch ON, then triggering the first toast, surfaces the desktop's notification mechanism (org.freedesktop.Notifications via D-Bus).
- [ ] Toast appears in the notification tray and respects the desktop's do-not-disturb mode.
- [ ] Per-trigger toggles silence their own category without affecting the other.
- [ ] No dock badge equivalent on Linux in v1 - documented gap.
- [ ] Clicking a toast focuses the window and deep-links to the relevant PR (verify on both X11 and Wayland - Wayland focus behaviour can differ).

### Sync behaviour

- [ ] Status bar shows "Last synced N ago" at all times.
- [ ] Manual refresh works from the sidebar control.
- [ ] Rate-limit guard pauses sync at 20% remaining; throttle state visible in the status bar.
- [ ] Activity feed shows phase progression.

### Window chrome

- [ ] Window controls (minimise / maximise / close) appear on the side the desktop environment expects (left on GNOME, right on KDE / XFCE).
- [ ] Tiling shortcuts (Super + arrow on GNOME, Meta + arrow on KDE) work normally.
- [ ] On a HiDPI display with fractional scaling (1.5x on Wayland): icons and text render crisply.
- [ ] Dragging between displays at different scales does not corrupt the layout.

### Theme

- [ ] First launch matches the desktop's colour scheme (GNOME `prefers-color-scheme`, KDE colour scheme).
- [ ] Flipping the system theme while PRism is running flips the UI live.

---

## Sign-off

- [ ] All three platforms passed.
- [ ] Failures filed as GitHub issues with the platform and step in the title.
- [ ] Release tag is safe to cut.
