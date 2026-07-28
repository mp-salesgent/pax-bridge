// Auto-update via electron-updater, backed by GitHub Releases.
//
// Publishing (see electron-builder.yml → publish) uploads the installer plus a
// latest.yml / latest-mac.yml feed to a GitHub Release; electron-updater reads
// that feed, downloads the delta, and installs on quit. All lifecycle events
// are forwarded to the renderer so the UI can show progress.
const { autoUpdater } = require('electron-updater');
const { app } = require('electron');

// Turn whatever electron-updater throws (often a multi-line HTTP error dump
// with headers/cookies) into one short, human sentence for the toast.
function friendlyMessage(err) {
  const raw = err == null ? 'Unknown error' : err.message || String(err);
  const firstLine = raw.split('\n')[0];
  if (/404/.test(firstLine)) return 'No update available yet — no release has been published.';
  if (/ENOTFOUND|ETIMEDOUT|ECONNREFUSED|network/i.test(firstLine)) return 'Could not reach the update server. Check your internet connection.';
  return firstLine.length > 140 ? `${firstLine.slice(0, 140)}…` : firstLine;
}

function initUpdater({ send, getSettings }) {
  autoUpdater.autoDownload = false; // let the user click "Download"
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.logger = { info: console.log, warn: console.warn, error: console.error, debug: () => {} };

  const emit = (type, payload = {}) => send('update:event', { type, ...payload });
  let checking = false;

  autoUpdater.on('checking-for-update', () => emit('checking'));
  autoUpdater.on('update-available', (info) => emit('available', { version: info.version, notes: info.releaseNotes }));
  autoUpdater.on('update-not-available', () => emit('none', { current: app.getVersion() }));
  autoUpdater.on('error', (err) => {
    // Full detail stays in the app log; the UI only ever sees one short line.
    console.error('[updater]', err);
    emit('error', { message: friendlyMessage(err) });
  });
  autoUpdater.on('download-progress', (p) =>
    emit('progress', { percent: Math.round(p.percent), bytesPerSecond: p.bytesPerSecond, transferred: p.transferred, total: p.total }),
  );
  autoUpdater.on('update-downloaded', (info) => emit('downloaded', { version: info.version }));

  // Kick off a check shortly after launch when the user has auto-update on —
  // silently: if there's no release yet or the network is down, don't show
  // an alarming error toast for a check the user never asked for.
  const maybeAutoCheck = () => {
    if (getSettings().autoUpdate && app.isPackaged) {
      autoUpdater.checkForUpdates().catch(() => {});
    }
  };
  setTimeout(maybeAutoCheck, 4000);

  return {
    check: () => {
      if (!app.isPackaged) {
        emit('dev', { message: 'Updates only run in a packaged build.' });
        return;
      }
      if (checking) return;
      checking = true;
      autoUpdater
        .checkForUpdates()
        .catch((e) => emit('error', { message: friendlyMessage(e) }))
        .finally(() => { checking = false; });
    },
    download: () => autoUpdater.downloadUpdate().catch((e) => emit('error', { message: friendlyMessage(e) })),
    quitAndInstall: () => autoUpdater.quitAndInstall(),
  };
}

module.exports = { initUpdater };
