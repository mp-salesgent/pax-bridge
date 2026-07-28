# Salesgent Pax Bridge — Deployment Guide

How to build the installers (`.dmg` for macOS, `.exe` for Windows) and ship
auto-updating releases to customers.

- [1. One-time setup](#1-one-time-setup)
- [2. Build the macOS installer (.dmg)](#2-build-the-macos-installer-dmg)
- [3. Build the Windows installer (.exe)](#3-build-the-windows-installer-exe)
- [4. Ship auto-updates (GitHub Releases)](#4-ship-auto-updates-github-releases)
- [5. Release with CI (mac + Windows together)](#5-release-with-ci-mac--windows-together)
- [6. What the customer does](#6-what-the-customer-does)
- [7. Versioning & how updates reach customers](#7-versioning--how-updates-reach-customers)
- [8. Troubleshooting](#8-troubleshooting)

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
> You'll see `skipped macOS code signing` — expected for unsigned builds
> (see [§8](#8-troubleshooting) to sign/notarize).

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
builds mac + Windows on their own native runners (no wine) and publishes both
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

Each runner builds its native installer (`.dmg` on macOS, `.exe` on Windows) and
uploads to the same GitHub Release.

---

## 6. What the customer does

**macOS** — download the `.dmg` for their chip → open it → drag **Salesgent Pax Bridge**
to Applications → first launch: right-click the app → **Open** (unsigned app).

**Windows** — download `PAX-Bridge-Setup-<ver>.exe` → run it → on the SmartScreen
prompt click **More info → Run anyway** → finish the wizard → launch from the
Start Menu / desktop.

The app keeps running in the menu bar / system tray. In **Settings** they can
turn on *Launch at login* and *Start bridge on launch* so it's always ready.

---

## 7. Versioning & how updates reach customers

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

## 8. Troubleshooting

| Symptom | Fix |
|---------|-----|
| `skipped macOS code signing` | Expected for unsigned builds. To remove the right-click-to-open step, set `mac.identity` to your Apple Developer ID in `electron-builder.yml` and add notarization (`afterSign` + `@electron/notarize`). |
| Windows "Windows protected your PC" (SmartScreen) | Expected for unsigned `.exe`. Buy an EV/OV code-signing cert and set `win.certificateFile` / `certificatePassword` (or use Azure Trusted Signing). |
| `dist:win` fails on macOS | Install `wine`, or build on Windows / CI ([§3](#3-build-the-windows-installer-exe), [§5](#5-release-with-ci-mac--windows-together)). |
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
