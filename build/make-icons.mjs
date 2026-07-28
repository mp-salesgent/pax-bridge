/**
 * Generate every app icon electron-builder needs, with zero third-party deps:
 *   build/icon.png   (1024) — Linux + window icon
 *   build/icon.icns          — macOS (via iconutil/sips, mac only)
 *   build/icon.ico           — Windows (pure-Node ICO packer)
 *   build/trayTemplate.png   — macOS menu-bar template icon
 *
 * We hand-roll a tiny RGBA PNG encoder (node:zlib deflates), draw a "payment
 * terminal" tile into a pixel buffer, and pack PNGs into .ico / .icns.
 *
 * Run:  npm run icons
 */
import { deflateSync } from 'node:zlib';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const OUT = path.dirname(fileURLToPath(import.meta.url));

// --- PNG encoder -----------------------------------------------------------
const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
const crc32 = (b) => {
  let c = 0xffffffff;
  for (let i = 0; i < b.length; i++) c = CRC[(c ^ b[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(type, 'ascii');
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(Buffer.concat([t, data])), 0);
  return Buffer.concat([len, t, data, crc]);
};
function encodePng(w, h, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 6;
  const stride = w * 4;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) { raw[y * (stride + 1)] = 0; rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride); }
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

// --- drawing ---------------------------------------------------------------
function drawTile(size) {
  const buf = Buffer.alloc(size * size * 4);
  const set = (x, y, r, g, b, a) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    const i = (y * size + x) * 4, sa = a / 255;
    buf[i] = Math.round(r * sa + buf[i] * (1 - sa));
    buf[i + 1] = Math.round(g * sa + buf[i + 1] * (1 - sa));
    buf[i + 2] = Math.round(b * sa + buf[i + 2] * (1 - sa));
    buf[i + 3] = Math.min(255, buf[i + 3] + a);
  };
  const inRR = (x, y, x0, y0, x1, y1, rad) => {
    if (x < x0 || y < y0 || x >= x1 || y >= y1) return false;
    const cx = x < x0 + rad ? x0 + rad : x > x1 - rad ? x1 - rad : x;
    const cy = y < y0 + rad ? y0 + rad : y > y1 - rad ? y1 - rad : y;
    return (x - cx) ** 2 + (y - cy) ** 2 <= rad * rad;
  };
  const pad = Math.round(size * 0.08), rad = Math.round(size * 0.22);
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
    if (!inRR(x, y, pad, pad, size - pad, size - pad, rad)) continue;
    const t = (x + y) / (2 * size);
    set(x, y, Math.round(79 + t * 45), Math.round(70 - t * 12), Math.round(229 + t * 8), 255);
  }
  const x0 = Math.round(size * 0.26), x1 = Math.round(size * 0.74), y0 = Math.round(size * 0.34), y1 = Math.round(size * 0.66);
  for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) if (inRR(x, y, x0, y0, x1, y1, Math.round(size * 0.04))) set(x, y, 255, 255, 255, 255);
  const sy0 = y0 + Math.round(size * 0.045), sy1 = sy0 + Math.round(size * 0.05);
  for (let y = sy0; y < sy1; y++) for (let x = x0; x < x1; x++) set(x, y, 45, 45, 60, 255);
  const cx0 = x0 + Math.round(size * 0.04), cy0 = sy1 + Math.round(size * 0.03), cs = Math.round(size * 0.06);
  for (let y = cy0; y < cy0 + cs; y++) for (let x = cx0; x < cx0 + cs; x++) set(x, y, 210, 180, 90, 255);
  return buf;
}

// Monochrome template for the macOS menu bar (black shape on transparent).
function drawTemplate(size) {
  const buf = Buffer.alloc(size * size * 4);
  const set = (x, y, a) => { const i = (y * size + x) * 4; buf[i + 3] = a; };
  const x0 = Math.round(size * 0.16), x1 = Math.round(size * 0.84), y0 = Math.round(size * 0.26), y1 = Math.round(size * 0.74);
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) {
    const border = x >= x0 && x < x1 && y >= y0 && y < y1;
    const inner = x >= x0 + 2 && x < x1 - 2 && y >= y0 + 2 && y < y1 - 2;
    const stripe = inner && y >= y0 + Math.round(size * 0.1) && y < y0 + Math.round(size * 0.2);
    if ((border && !inner) || stripe) set(x, y, 255);
  }
  return buf;
}

// --- ICO packer ------------------------------------------------------------
function encodeIco(pngs) {
  // pngs: [{ size, buf }]. ICO with PNG-compressed images (Vista+).
  const count = pngs.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); header.writeUInt16LE(1, 2); header.writeUInt16LE(count, 4);
  const entries = Buffer.alloc(16 * count);
  let offset = 6 + 16 * count;
  const bodies = [];
  pngs.forEach((p, i) => {
    const e = entries.subarray(i * 16, i * 16 + 16);
    e[0] = p.size >= 256 ? 0 : p.size;
    e[1] = p.size >= 256 ? 0 : p.size;
    e[2] = 0; e[3] = 0;
    e.writeUInt16LE(1, 4); e.writeUInt16LE(32, 6);
    e.writeUInt32LE(p.buf.length, 8); e.writeUInt32LE(offset, 12);
    offset += p.buf.length; bodies.push(p.buf);
  });
  return Buffer.concat([header, entries, ...bodies]);
}

// --- write outputs ---------------------------------------------------------
function scaledPng(size) { return encodePng(size, size, drawTile(size)); }

fs.writeFileSync(path.join(OUT, 'icon.png'), scaledPng(1024));
fs.writeFileSync(path.join(OUT, 'trayTemplate.png'), encodePng(36, 36, drawTemplate(36)));
// @2x for retina menu bars
fs.writeFileSync(path.join(OUT, 'trayTemplate@2x.png'), encodePng(72, 72, drawTemplate(72)));

const icoSizes = [16, 32, 48, 64, 128, 256];
fs.writeFileSync(path.join(OUT, 'icon.ico'), encodeIco(icoSizes.map((s) => ({ size: s, buf: scaledPng(s) }))));

// .icns needs macOS tooling; skip elsewhere (electron-builder can derive from png).
if (process.platform === 'darwin') {
  const iconset = path.join(OUT, 'icon.iconset');
  fs.rmSync(iconset, { recursive: true, force: true });
  fs.mkdirSync(iconset, { recursive: true });
  const master = path.join(OUT, 'icon.png');
  const spec = [
    [16, 'icon_16x16.png'], [32, 'icon_16x16@2x.png'], [32, 'icon_32x32.png'],
    [64, 'icon_32x32@2x.png'], [128, 'icon_128x128.png'], [256, 'icon_128x128@2x.png'],
    [256, 'icon_256x256.png'], [512, 'icon_256x256@2x.png'], [512, 'icon_512x512.png'],
    [1024, 'icon_512x512@2x.png'],
  ];
  for (const [px, name] of spec) spawnSync('sips', ['-z', String(px), String(px), master, '--out', path.join(iconset, name)], { stdio: 'ignore' });
  const r = spawnSync('iconutil', ['-c', 'icns', iconset, '-o', path.join(OUT, 'icon.icns')], { stdio: 'inherit' });
  fs.rmSync(iconset, { recursive: true, force: true });
  if (r.status === 0) console.log('✓ icon.icns');
}

console.log('✓ icon.png, icon.ico, trayTemplate.png');
