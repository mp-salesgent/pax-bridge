// Bridges the renderer to the Rust/Tauri backend, replacing the old Electron
// preload script. Requires `app.withGlobalTauri = true` in tauri.conf.json so
// `window.__TAURI__.core` / `window.__TAURI__.event` are available without a
// bundler.
(() => {
  const invoke = (...args) => window.__TAURI__.core.invoke(...args);
  const listen = (channel, cb) => {
    let unlisten = () => {};
    window.__TAURI__.event.listen(channel, (event) => cb(event.payload)).then((fn) => (unlisten = fn));
    return () => unlisten();
  };

  window.pax = {
    app: {
      info: () => invoke('app_info'),
      quit: () => invoke('quit_app'),
      openUserData: () => invoke('open_user_data'),
      openExternal: (url) => invoke('open_external', { url }),
    },
    bridge: {
      state: () => invoke('bridge_state'),
      logs: () => invoke('bridge_logs'),
      start: () => invoke('bridge_start'),
      stop: () => invoke('bridge_stop'),
      restart: () => invoke('bridge_restart'),
      onLog: (cb) => listen('bridge:log', cb),
      onStatus: (cb) => listen('bridge:status', cb),
    },
    logs: {
      download: () => invoke('logs_download'),
    },
    settings: {
      get: () => invoke('settings_get'),
      set: (patch) => invoke('settings_set', { patch }),
    },
    updates: {
      check: () => invoke('update_check'),
      download: () => invoke('update_download'),
      install: () => invoke('update_install'),
      onEvent: (cb) => listen('update:event', cb),
    },
  };
})();
