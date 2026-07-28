/**
 * Ad-hoc re-sign the macOS .app after electron-builder assembles it.
 *
 * Why this is required (not optional):
 * Electron's prebuilt binary ships with a "linker-signed" ad-hoc signature.
 * electron-builder then renames the executable, injects our resources and
 * rewrites Info.plist — which invalidates that signature. With
 * `mac.identity: null` electron-builder skips signing entirely, so the app is
 * shipped carrying a signature that no longer matches its contents. macOS
 * refuses to launch it with the misleading error:
 *
 *     "Salesgent Pax Bridge is damaged and can't be opened."
 *
 * Apple Silicon in particular hard-requires a *valid* signature (ad-hoc is
 * enough) — an invalid one is worse than none. Re-signing here makes the
 * signature match the real contents, so the app launches after the normal
 * right-click → Open for unsigned developers.
 *
 * This is not a substitute for a real Developer ID + notarization (which would
 * remove the right-click step entirely) — it's the minimum needed to make an
 * unsigned build actually runnable.
 */
const { execFileSync } = require('node:child_process');
const path = require('node:path');

exports.default = async function afterPack(context) {
  if (context.electronPlatformName !== 'darwin') return;

  const appName = `${context.packager.appInfo.productFilename}.app`;
  const appPath = path.join(context.appOutDir, appName);

  console.log(`  • ad-hoc signing ${appName}`);
  try {
    // --deep so nested frameworks, helper apps and native .node addons (e.g.
    // serialport's prebuilds inside the bridge) are signed too.
    execFileSync('codesign', ['--force', '--deep', '--sign', '-', appPath], {
      stdio: 'inherit',
    });
    // Fail the build rather than shipping another "damaged" app.
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
};
