// Safe bridge between the sandboxed renderer and the main process.
// Only the explicit methods below are exposed — no Node, no ipcRenderer.
const { contextBridge, ipcRenderer } = require('electron');

const on = (channel) => (cb) => {
  const listener = (_e, payload) => cb(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
};

contextBridge.exposeInMainWorld('pax', {
  app: {
    info: () => ipcRenderer.invoke('app:info'),
    quit: () => ipcRenderer.invoke('app:quit'),
    openUserData: () => ipcRenderer.invoke('open:userData'),
    openExternal: (url) => ipcRenderer.invoke('open:external', url),
  },
  bridge: {
    state: () => ipcRenderer.invoke('bridge:state'),
    logs: () => ipcRenderer.invoke('bridge:logs'),
    start: (port) => ipcRenderer.invoke('bridge:start', port),
    stop: () => ipcRenderer.invoke('bridge:stop'),
    restart: (port) => ipcRenderer.invoke('bridge:restart', port),
    onLog: on('bridge:log'),
    onStatus: on('bridge:status'),
  },
  logs: {
    download: () => ipcRenderer.invoke('logs:download'),
  },
  settings: {
    get: () => ipcRenderer.invoke('settings:get'),
    set: (patch) => ipcRenderer.invoke('settings:set', patch),
  },
  updates: {
    check: () => ipcRenderer.invoke('update:check'),
    download: () => ipcRenderer.invoke('update:download'),
    install: () => ipcRenderer.invoke('update:install'),
    onEvent: on('update:event'),
  },
});
