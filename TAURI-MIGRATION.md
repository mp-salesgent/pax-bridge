# Tauri + Rust migration (`pax-rust`)

Electron shell + Node bridge → Tauri 2 + in-process Rust bridge (axum).

## Size (local arm64 release build)

| Artifact | Size |
|----------|------|
| Electron DMG (previous) | ~92–104 MB |
| Tauri `.app` | **~6.1 MB** |
| Tauri binary | **~6.0 MB** |

## Dev / build

```bash
npm install
npm run dev      # tauri dev
npm run build    # tauri build
npm run build:dmg
```

## Notes

- Node `bridge/` and Electron `src/main` kept for reference; runtime uses `src-tauri/src/bridge/`.
- Auto-updater pubkey is inactive until `TAURI_SIGNING_PRIVATE_KEY` is generated and set.
- CI: `.github/workflows/release-tauri.yml`
