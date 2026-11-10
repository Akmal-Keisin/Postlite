// Dependency-free icon generation: hand-rolled PNG (zlib) + ICO encoders so
// the project needs no npm packages. Linux bundles require the PNG set; Windows
// uses icon.ico.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const iconsDir = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons");

function insideRounded(x, y, size, radius) {
  const right = size - 1;
  const cornerX = x < radius ? radius : x > right - radius ? right - radius : x;
  const cornerY = y < radius ? radius : y > right - radius ? right - radius : y;
  const dx = x - cornerX;
  const dy = y - cornerY;
  return x >= 0 && y >= 0 && x <= right && y <= right && dx * dx + dy * dy <= radius * radius;
}

function inTriangle(x, y, [a, b, c]) {
  const sign = (p, q, r) => (p[0] - r[0]) * (q[1] - r[1]) - (p[1] - r[1]) * (q[0] - r[0]);
  const p = [x, y];
  const d1 = sign(p, a, b);
  const d2 = sign(p, b, c);
  const d3 = sign(p, c, a);
  const hasNeg = d1 < 0 || d2 < 0 || d3 < 0;
  const hasPos = d1 > 0 || d2 > 0 || d3 > 0;
  return !(hasNeg && hasPos);
}

function render(size) {
  const s = size / 32;
  const radius = 7 * s;
  const plane = [[7 * s, 15 * s], [25 * s, 6 * s], [18 * s, 26 * s]];
  const buf = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const i = (y * size + x) * 4;
      if (!insideRounded(x, y, size, radius)) {
        buf.writeUInt32LE(0, i);
        continue;
      }
      const t = (x + y) / (2 * (size - 1));
      let r = Math.round(79 + (125 - 79) * t);
      let g = Math.round(140 + (92 - 140) * t);
      let b = 255;
      if (inTriangle(x, y, plane)) {
        r = g = b = 248;
      }
      buf[i] = r;
      buf[i + 1] = g;
      buf[i + 2] = b;
      buf[i + 3] = 255;
    }
  }
  return buf;
}

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i += 1) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const t = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])), 0);
  return Buffer.concat([len, t, data, crc]);
}

function encodePNG(size, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const stride = size * 4;
  const raw = Buffer.alloc(size * (stride + 1));
  for (let y = 0; y < size; y += 1) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([sig, chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw, { level: 9 })), chunk("IEND", Buffer.alloc(0))]);
}

function encodeICO() {
  const SIZE = 32;
  const rgba = render(SIZE);
  const pixels = Buffer.alloc(SIZE * SIZE * 4);
  for (let y = 0; y < SIZE; y += 1) {
    const row = SIZE - 1 - y;
    for (let x = 0; x < SIZE; x += 1) {
      const si = (y * SIZE + x) * 4;
      const di = (row * SIZE + x) * 4;
      pixels[di] = rgba[si + 2];
      pixels[di + 1] = rgba[si + 1];
      pixels[di + 2] = rgba[si];
      pixels[di + 3] = rgba[si + 3];
    }
  }
  const mask = Buffer.alloc((SIZE / 8) * SIZE);
  const dib = Buffer.alloc(40);
  dib.writeUInt32LE(40, 0);
  dib.writeInt32LE(SIZE, 4);
  dib.writeInt32LE(SIZE * 2, 8);
  dib.writeUInt16LE(1, 12);
  dib.writeUInt16LE(32, 14);
  dib.writeUInt32LE(0, 16);
  dib.writeUInt32LE(pixels.length + mask.length, 20);
  const image = Buffer.concat([dib, pixels, mask]);
  const dir = Buffer.alloc(22);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(1, 4);
  dir[6] = SIZE;
  dir[7] = SIZE;
  dir.writeUInt16LE(1, 10);
  dir.writeUInt16LE(32, 12);
  dir.writeUInt32LE(image.length, 14);
  dir.writeUInt32LE(22, 18);
  return Buffer.concat([dir, image]);
}

mkdirSync(iconsDir, { recursive: true });
const pngs = { "32x32.png": 32, "128x128.png": 128, "128x128@2x.png": 256, "icon.png": 512 };
for (const [name, size] of Object.entries(pngs)) {
  writeFileSync(join(iconsDir, name), encodePNG(size, render(size)));
  console.log(name, `${size}x${size}`);
}
writeFileSync(join(iconsDir, "icon.ico"), encodeICO());
console.log("icon.ico 32x32");