// Dead-simple JSON settings store in userData/settings.json.
// No dependency — this is the only persistent config the desktop shell needs.
const fs = require('node:fs');
const path = require('node:path');
const { app } = require('electron');

const FILE = path.join(app.getPath('userData'), 'settings.json');

const DEFAULTS = {
  port: 5000,
  launchAtLogin: false,
  autoUpdate: true,
  startBridgeOnLaunch: true,
  minimizeToTray: true,
};

function read() {
  try {
    return { ...DEFAULTS, ...JSON.parse(fs.readFileSync(FILE, 'utf8')) };
  } catch {
    return { ...DEFAULTS };
  }
}

function write(patch) {
  const next = { ...read(), ...patch };
  try {
    fs.mkdirSync(path.dirname(FILE), { recursive: true });
    fs.writeFileSync(FILE, JSON.stringify(next, null, 2));
  } catch (err) {
    console.error('[store] failed to persist settings:', err.message);
  }
  return next;
}

module.exports = { read, write, DEFAULTS, FILE };
