# Salesgent Pax Bridge — Deployment Guide

How to build the installers (`.dmg` for macOS, `.exe` for Windows) and ship
auto-updating releases to customers.

- [1. One-time setup](#1-one-time-setup)
- [2. Build the macOS installer (.dmg)](#2-build-the-macos-installer-dmg)
- [3. Build the Windows installer (.exe)](#3-build-the-windows-installer-exe)
- [4. Ship auto-updates (GitHub Releases)](#4-ship-auto-updates-github-releases)
- [5. Release with CI (mac + Windows together)](#5-release-with-ci-mac--windows-together)
- [6. macOS signing & notarization (required for clients)](#6-macos-signing--notarization-required-for-clients)
- [7. What the customer does](#7-what-the-customer-does)
- [8. Versioning & how updates reach customers](#8-versioning--how-updates-reach-customers)
- [9. Troubleshooting](#9-troubleshooting)

---

## 1. One-time setup

**Requirements**

| Tool | For | Notes |
|------|-----|-------|
| Node 18+ / npm | everything | matches the repo |
| macOS machine | building the `.dmg` | Apple only allows building/signing mac apps on macOS |
| Windows machine **or** CI | building the `.exe` reliably | see §3 / §5 |
| `GH_TOKEN` (GitHub PAT, `repo` scope) | publishing auto-updates | only for `npm run release` |

**Install dependencies (once):**

```bash
cd ~/Desktop/pax-bridge-desktop
npm install              # app deps: electron, electron-builder, electron-updater
npm run bridge:install   # the embedded bridge's own deps (express, serialport, …)
```

That's it — `npm run dist:*` regenerates icons and refreshes bridge deps
automatically before every build (via the `prepare:build` script).

---

## 2. Build the macOS installer (.dmg)

```bash
cd ~/Desktop/pax-bridge-desktop
npm run dist:mac
```

**What happens**

1. `prepare:build` → regenerates `build/` icons + installs bridge deps.
2. `electron-builder --mac` → builds `Salesgent Pax Bridge.app` for **arm64 and x64**,
   wraps each in a `.dmg`, and also emits a `.zip` (required for auto-update).

**Output** → `release/`

```
release/
  PAX-Bridge-1.0.0-arm64.dmg      ← Apple Silicon installer
  PAX-Bridge-1.0.0-x64.dmg        ← Intel installer
  PAX-Bridge-1.0.0-arm64.zip      ← used by the auto-updater
  PAX-Bridge-1.0.0-x64.zip
  PAX-Bridge-1.0.0-arm64.dmg.blockmap   ← enables small delta updates
  latest-mac.yml                  ← the update feed metadata
```

> First run downloads the Electron runtime (~100 MB per arch), then it's cached.
> Without Apple signing secrets this is an unsigned local build (Gatekeeper will
> block clients). For customer-facing builds use CI with secrets — see [§6](#6-macos-signing--notarization-required-for-clients).

**Just one architecture (faster):**

```bash
npx electron-builder --mac dmg --arm64     # Apple Silicon only
npx electron-builder --mac dmg --x64       # Intel only
```

---

## 3. Build the Windows installer (.exe)

The Windows target produces an **NSIS `setup.exe`** (install wizard, Start-Menu
+ desktop shortcuts, uninstaller).

### Best: build on Windows
```bat
cd pax-bridge-desktop
npm install
npm run bridge:install
npm run dist:win
```

**Output** → `release/PAX-Bridge-Setup-1.0.0.exe` (+ `latest.yml`, `.blockmap`).

### From macOS/Linux
Cross-building the Windows installer needs **wine**:
```bash
brew install --cask wine-stable   # macOS
npm run dist:win
```
This can be flaky depending on OS version. If it fails, use CI ([§5](#5-release-with-ci-mac--windows-together)) —
that's the reliable path and builds mac + Windows in one go.

---

## 4. Ship auto-updates (GitHub Releases)

Auto-update is already wired in the app (`electron-updater` →
`src/main/updater.js`). It reads new versions from a GitHub Release feed.

### a) Point at your releases repo

Edit [`electron-builder.yml`](electron-builder.yml):

```yaml
publish:
  provider: github
  owner: salesgent            # ← your GitHub org/user
  repo: pax-bridge-desktop    # ← create this repo (or use an existing one)
```

> The releases repo can be public or private. For a **private** repo, customers'
> apps also need the token — simplest is to keep the releases repo public while
> keeping the source private, or switch to a generic S3/HTTPS feed later.

### b) Set a token

```bash
export GH_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxx   # GitHub PAT with "repo" scope
```

### c) Bump the version, then publish

```bash
# 1. bump "version" in package.json (e.g. 1.0.0 → 1.0.1)
# 2. build + upload installer + latest*.yml to a GitHub Release:
npm run release
```

`npm run release` = `electron-builder --publish always`. It creates (or updates)
a **draft** GitHub Release tagged `v<version>` and uploads the installers and the
`latest.yml` / `latest-mac.yml` update feeds. Publish the draft in GitHub when
you're ready for customers to receive it.

---

## 5. Release with CI (mac + Windows together)

Already set up — [`.github/workflows/release.yml`](.github/workflows/release.yml)
builds mac + Windows on `macos-latest` (Windows NSIS is cross-built there) and
publishes both to one GitHub Release.
to the same GitHub Release, triggered by pushing a `v*` tag or manually from
the Actions tab (`workflow_dispatch`):

```yaml
name: Release
on:
  push:
    tags: ["v*"]
  workflow_dispatch:
permissions:
  contents: write   # lets the built-in GITHUB_TOKEN publish the Release
jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - run: npm install
      - run: npm run bridge:install
      - run: npm run release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

> **One-time repo setting**: if the workflow fails with a 403/permission error
> on publish, go to **Settings → Actions → General → Workflow permissions** and
> select **"Read and write permissions"**. The `permissions: contents: write`
> block above requests it, but a repo-level setting can still cap it.

Then release by pushing a tag:

```bash
# after bumping "version" in package.json to 1.0.1
git commit -am "release 1.0.1"
git tag v1.0.1
git push origin main --tags
```

Each runner builds its installer and uploads to the same GitHub Release.
macOS builds **must** have signing + notarization secrets (see §6) or the
publish job fails — by design, so clients never get a Gatekeeper-blocked app.

---

## 6. macOS signing & notarization (required for clients)

Without this, macOS shows **"Apple could not verify … is free of malware"** and
the only workaround is a right-click → Open on each client PC. That is not
acceptable for production.

You need an **Apple Developer Program** membership ($99/year).

### a) Create a Developer ID Application certificate

1. On a Mac, open **Keychain Access** → Certificate Assistant → Request a Certificate From a Certificate Authority (save to disk).
2. In [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/list) create **Developer ID Application** and upload the CSR.
3. Download the `.cer`, double-click to install into Keychain.
4. Export it as a `.p12` (include private key), set a strong password.

Encode for GitHub:

```bash
base64 -i "Developer ID Application.p12" | pbcopy
```

### b) Create notarization credentials (pick one)

**Option A — App Store Connect API key (recommended)**

1. [App Store Connect → Users and Access → Integrations → Team Keys](https://appstoreconnect.apple.com/access/integrations/api)
2. Create a key with **Developer** access, download `AuthKey_XXXXXX.p8` (once only).
3. Note **Key ID** and **Issuer ID** (UUID).

Store the `.p8` contents (or base64 of the file) as the `APPLE_API_KEY` secret.

**Option B — Apple ID + app-specific password**

1. Apple ID → Sign-In and Security → App-Specific Passwords → generate one.
2. Find your **Team ID** at [developer.apple.com/account](https://developer.apple.com/account) (Membership details).

### c) Add GitHub repo secrets

Repo → **Settings → Secrets and variables → Actions** → add:

| Secret | Value |
|--------|--------|
| `CSC_LINK` | base64 of the `.p12` (Developer ID Application) |
| `CSC_KEY_PASSWORD` | password used when exporting the `.p12` |
| **Either API key** | |
| `APPLE_API_KEY` | raw `.p8` PEM text **or** base64 of the file |
| `APPLE_API_KEY_ID` | Key ID (e.g. `AB12CD34EF`) |
| `APPLE_API_ISSUER` | Issuer UUID |
| **Or Apple ID** | |
| `APPLE_ID` | your Apple ID email |
| `APPLE_APP_SPECIFIC_PASSWORD` | app-specific password |
| `APPLE_TEAM_ID` | 10-character Team ID |

Tag releases (`v*`) then fail the mac job if these are missing — so an
unsigned build cannot ship by accident.

### d) What CI does with them

1. Imports `CSC_LINK` → signs the `.app` with **Developer ID** + hardened runtime.
2. Submits to Apple **notarytool** and staples the ticket.
3. Packages signed/notarized `.dmg` / `.zip`.

Clients open the app with a normal double-click — no Gatekeeper block.

---

## 7. What the customer does

**macOS** — download the `.dmg` for their chip → open it → drag **Salesgent Pax Bridge**
to Applications → **double-click to open** (signed + notarized builds).

**Windows** — download `PAX-Bridge-Setup.exe` → run it → if SmartScreen appears
click **More info → Run anyway** (Windows code signing is separate; see §9) →
finish the wizard → launch from the Start Menu / desktop.

The app keeps running in the menu bar / system tray. In **Settings** they can
turn on *Launch at login* and *Start bridge on launch* so it's always ready.

---

## 8. Versioning & how updates reach customers

- The single source of truth is **`version`** in `package.json`.
- Update semver every release: patch (`1.0.1`) for fixes, minor (`1.1.0`) for
  features.
- After you publish a Release, installed apps:
  1. check the feed on launch (and when the user clicks **Check for updates**),
  2. download the new version in the background (with a progress toast),
  3. install it on the next restart (**Restart & install** button, or on quit).
- Delta updates: the `.blockmap` means customers only download what changed, not
  the whole 90 MB each time.
- ⚠️ Auto-update only runs in a **packaged** build — never in `npm start`.

---

## 9. Troubleshooting

| Symptom | Fix |
|---------|-----|
| Gatekeeper: "Apple could not verify…" | Build was not signed/notarized. Add §6 secrets and cut a new tag release. |
| CI: `Missing secret CSC_LINK` | Export Developer ID `.p12`, base64 it, add as `CSC_LINK` (+ `CSC_KEY_PASSWORD`). |
| CI: notarization / notarytool errors | Check API key or Apple ID secrets; Team ID must match the signing cert's team. |
| Windows "Windows protected your PC" (SmartScreen) | Expected for unsigned `.exe`. Buy an EV/OV code-signing cert and configure `win` signing (separate from macOS). |
| `dist:win` fails on macOS | Use CI ([§5](#5-release-with-ci-mac--windows-together)) — Windows NSIS is built on macOS runners there. |
| `ENOENT app-update.yml` in logs | Only happens with `npm run pack` (`--dir`). Real `dist:*` builds generate it — ignore for unpacked smoke tests. |
| USB terminal not detected in the packaged app | `electron-builder` rebuilds `serialport` for you; if USB still fails, run `npx @electron/rebuild -m bridge` before `dist`. LAN/TCP always works and is the default. |
| "Port already in use" on launch | Another bridge is on that port. Change the port in **Settings** (restarts the bridge automatically). |
| Update never appears | Confirm the GitHub Release is **published** (not draft), the new `version` is higher, and `publish.owner/repo` match the release repo. |

---

### Quick reference

```bash
npm start            # run in dev (no installer)
npm run pack         # unpacked app in release/ (fast smoke test)
npm run dist:mac     # .dmg (arm64 + x64)
npm run dist:win     # .exe (NSIS) — Windows or wine
npm run dist:all     # mac + win + linux
npm run release      # build + upload to GitHub Releases (needs GH_TOKEN)
```
