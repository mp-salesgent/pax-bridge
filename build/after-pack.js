/**
 * Post-pack cleanup + macOS ad-hoc re-sign.
 *
 * 1) Trim Electron runtime dead weight (licenses, extra locales, Vulkan
 *    software rasterizer this POS bridge never uses).
 * 2) Drop serialport prebuilds for other OSes (Android/Linux/wrong Windows
 *    arch) so they aren't shipped inside every installer.
 * 3) On macOS, re-sign the .app ad-hoc so Gatekeeper doesn't report it as
 *    "damaged" after electron-builder rewrote the Electron stub.
 */
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

function rm(p, stats) {
  try {
    const st = fs.statSync(p);
    fs.rmSync(p, { recursive: true, force: true });
    stats.saved += st.size || 0;
    stats.count += 1;
  } catch {
    /* absent on this platform — fine */
  }
}

function resourcesDir(context) {
  if (context.electronPlatformName === 'darwin') {
    const appName = `${context.packager.appInfo.productFilename}.app`;
    return path.join(context.appOutDir, appName, 'Contents', 'Resources');
  }
  return path.join(context.appOutDir, 'resources');
}

/**
 * Keep only the native prebuild matching this build's OS/arch.
 * serialport ships android/linux/win32-ia32/… (~1.5 MB of dead weight).
 */
function trimBridgePrebuilds(context, stats) {
  const prebuilds = path.join(
    resourcesDir(context),
    'bridge',
    'node_modules',
    '@serialport',
    'bindings-cpp',
    'prebuilds',
  );
  if (!fs.existsSync(prebuilds)) return;

  const platform = context.electronPlatformName; // darwin | win32 | linux
  // builder-util Arch enum: ia32=0, x64=1, armv7l=2, arm64=3
  const archName = { 0: 'ia32', 1: 'x64', 2: 'arm', 3: 'arm64' }[context.arch] || 'x64';

  let keep;
  if (platform === 'darwin') {
    // universal prebuild covers both intel + apple silicon
    keep = new Set(['darwin-x64+arm64']);
  } else if (platform === 'win32') {
    keep = new Set([`win32-${archName}`]);
  } else {
    keep = new Set([`linux-${archName}`]);
  }

  for (const dir of fs.readdirSync(prebuilds)) {
    if (!keep.has(dir)) rm(path.join(prebuilds, dir), stats);
  }
}

/**
 * Strip Electron files this app never needs. Chromium itself (~200 MB) is
 * non-negotiable; these are optional add-ons.
 */
function trimRuntime(context, stats) {
  const dir = context.appOutDir;

  rm(path.join(dir, 'LICENSES.chromium.html'), stats);
  rm(path.join(dir, 'LICENSE.electron.txt'), stats);
  rm(path.join(dir, 'version'), stats);

  // Windows/Linux locale packs (macOS handled by electronLanguages)
  const locales = path.join(dir, 'locales');
  if (fs.existsSync(locales)) {
    for (const f of fs.readdirSync(locales)) {
      if (f !== 'en-US.pak') rm(path.join(locales, f), stats);
    }
  }

  // Vulkan software stack — unused by a POS bridge UI (~5–15 MB depending on OS)
  for (const name of [
    'vk_swiftshader.dll',
    'vk_swiftshader_icd.json',
    'vulkan-1.dll',
    'libvk_swiftshader.dylib',
    'libvk_swiftshader.so',
    'vk_swiftshader_icd.json',
    'libvulkan.so.1',
    'libvulkan.dylib',
  ]) {
    rm(path.join(dir, name), stats);
  }

  // Nested under Electron Framework on macOS
  if (context.electronPlatformName === 'darwin') {
    const appName = `${context.packager.appInfo.productFilename}.app`;
    const fw = path.join(
      dir,
      appName,
      'Contents',
      'Frameworks',
      'Electron Framework.framework',
      'Versions',
      'A',
      'Libraries',
    );
    for (const name of ['libvk_swiftshader.dylib', 'vk_swiftshader_icd.json']) {
      rm(path.join(fw, name), stats);
    }
    // Also drop non-English .lproj leftovers if any slipped past electronLanguages
    const resources = path.join(dir, appName, 'Contents', 'Resources');
    if (fs.existsSync(resources)) {
      for (const f of fs.readdirSync(resources)) {
        if (f.endsWith('.lproj') && f !== 'en.lproj' && f !== 'en_US.lproj') {
          rm(path.join(resources, f), stats);
        }
      }
    }
  }
}

function adHocSignMac(context) {
  const appName = `${context.packager.appInfo.productFilename}.app`;
  const appPath = path.join(context.appOutDir, appName);

  console.log(`  • ad-hoc signing ${appName}`);
  try {
    execFileSync('codesign', ['--force', '--deep', '--sign', '-', appPath], {
      stdio: 'inherit',
    });
    execFileSync('codesign', ['--verify', '--deep', '--strict', appPath], {
      stdio: 'inherit',
    });
    console.log('  • ad-hoc signature verified');
  } catch (err) {
    throw new Error(
      `Ad-hoc code signing failed for ${appName}: ${err.message}\n` +
        'Shipping without a valid signature makes macOS report the app as "damaged".',
    );
  }
}

exports.default = async function afterPack(context) {
  const stats = { saved: 0, count: 0 };

  trimRuntime(context, stats);
  trimBridgePrebuilds(context, stats);

  if (stats.saved > 0) {
    console.log(
      `  • trimmed ${stats.count} unused file(s)  saved=${(stats.saved / 1e6).toFixed(1)}MB`,
    );
  }

  if (context.electronPlatformName === 'darwin') {
    adHocSignMac(context);
  }
};
