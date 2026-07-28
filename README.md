# PAX Bridge — desktop app

An installable, **auto-updating** Electron desktop app that runs the PAX payment
bridge on a store computer. It wraps the Express bridge (`bridge/`) in a real
GUI with a system tray, terminal management, live logs, customer-facing options,
and one-click updates from GitHub Releases.

```
┌──────────── Electron ────────────┐
│  main process                    │
│   • window (renderer UI)         │
│   • system tray                  │
│   • auto-updater (GitHub)        │
│   • utilityProcess ──► bridge/index.js  (Express :5000, its own Node process)
└──────────────────────────────────┘
```

The bridge runs as a **separate Node process** (`utilityProcess.fork`), so a
payment-server crash can never take down the UI. All config + the JSON DB live
in the app's `userData` folder (`PAX_HOME`), never inside the app bundle.

## Develop

```bash
npm install            # app deps (electron, electron-builder, electron-updater)
npm run bridge:install # the bridge's own runtime deps (express, serialport, …)
npm run icons          # generate build/ icons (png/ico/icns/tray)
npm start              # launch in dev
```

## Build installers

```bash
npm run dist:mac       # → release/PAX-Bridge-<ver>-<arch>.dmg (+ zip for updates)
npm run dist:win       # → release/PAX-Bridge-Setup-<ver>.exe  (NSIS)
npm run pack           # unpacked app in release/ (fast smoke test, no installer)
```

> `dist:*` runs `prepare:build` first (icons + bridge deps). Building the Windows
> installer from macOS needs `wine`; the reliable path is CI (see below).

## Auto-updates (GitHub Releases)

1. Create a repo for releases and point `publish` in
   [`electron-builder.yml`](electron-builder.yml) at it (`owner` / `repo`).
2. Set a token: `export GH_TOKEN=<a PAT with repo scope>`.
3. Bump `version` in `package.json`, then:
   ```bash
   npm run release      # builds + uploads installer & latest*.yml to a Release
   ```
4. Installed apps check that feed on launch (and via **Check for updates**),
   download in the background, and install on restart — driven by
   [`src/main/updater.js`](src/main/updater.js). Updates only run in a packaged
   build, not in `npm start`.

### CI (recommended for Windows + mac together)

Run `electron-builder` on a matrix of `macos-latest` + `windows-latest` with
`GH_TOKEN` in the environment and `--publish always`. Each runner builds its
native installer and uploads to the same GitHub Release.

## Customer options

The **Settings** tab exposes the options a merchant can control: bridge port,
launch at login, start bridge on launch, keep running in tray, and automatic
updates. They persist to `userData/settings.json`.

## Notes

- **Code signing:** builds are unsigned. macOS → right-click → Open the first
  time; Windows → *More info → Run anyway*. Add an Apple Developer ID +
  notarization and a Windows cert in `electron-builder.yml` to remove the
  prompts.
- **USB terminals:** `serialport` is a native module. TCP/LAN terminals work out
  of the box. If USB isn't detected in a packaged build, rebuild serialport for
  Electron's ABI (`npx @electron/rebuild -m bridge`) before `dist`. LAN is
  unaffected and is the default connection type.
- **Bridge source:** `bridge/` is a copy of the ERP's `pax-server`. Keep them in
  sync, or later make `bridge/` a git submodule / npm dependency.
```
