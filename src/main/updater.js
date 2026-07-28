// Auto-update via electron-updater, backed by GitHub Releases.
//
// Publishing (see electron-builder.yml → publish) uploads the installer plus a
// latest.yml / latest-mac.yml feed to a GitHub Release; electron-updater reads
// that feed, downloads the delta, and installs on quit. All lifecycle events
// are forwarded to the renderer so the UI can show progress.
const { autoUpdater } = require('electron-updater');
const { app } = require('electron');

function initUpdater({ send, getSettings }) {
  autoUpdater.autoDownload = false; // let the user click "Download"
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.logger = { info: console.log, warn: console.warn, error: console.error, debug: () => {} };

  const emit = (type, payload = {}) => send('update:event', { type, ...payload });

  autoUpdater.on('checking-for-update', () => emit('checking'));
  autoUpdater.on('update-available', (info) => emit('available', { version: info.version, notes: info.releaseNotes }));
  autoUpdater.on('update-not-available', () => emit('none', { current: app.getVersion() }));
  autoUpdater.on('error', (err) => emit('error', { message: err == null ? 'unknown' : (err.message || String(err)) }));
  autoUpdater.on('download-progress', (p) =>
    emit('progress', { percent: Math.round(p.percent), bytesPerSecond: p.bytesPerSecond, transferred: p.transferred, total: p.total }),
  );
  autoUpdater.on('update-downloaded', (info) => emit('downloaded', { version: info.version }));

  // Kick off a check shortly after launch when the user has auto-update on.
  const maybeAutoCheck = () => {
    if (getSettings().autoUpdate && app.isPackaged) {
      autoUpdater.checkForUpdates().catch((e) => emit('error', { message: e.message }));
    }
  };
  setTimeout(maybeAutoCheck, 4000);

  return {
    check: () => {
      if (!app.isPackaged) {
        emit('dev', { message: 'Updates only run in a packaged build.' });
        return;
      }
      autoUpdater.checkForUpdates().catch((e) => emit('error', { message: e.message }));
    },
    download: () => autoUpdater.downloadUpdate().catch((e) => emit('error', { message: e.message })),
    quitAndInstall: () => autoUpdater.quitAndInstall(),
  };
}

module.exports = { initUpdater };
