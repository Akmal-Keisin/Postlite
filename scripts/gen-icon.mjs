import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 32;
const RADIUS = 7;
const out = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons", "icon.ico");

function insideRounded(x, y) {
  const right = SIZE - 1;
  const cornerX = x < RADIUS ? RADIUS : x > right - RADIUS ? right - RADIUS : x;
  const cornerY = y < RADIUS ? RADIUS : y > right - RADIUS ? right - RADIUS : y;
  const dx = x - cornerX;
  const dy = y - cornerY;
  return x >= 0 && y >= 0 && x <= right && y <= right && dx * dx + dy * dy <= RADIUS * RADIUS;
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

const plane = [
  [7, 15],
  [25, 6],
  [18, 26],
];

// Bottom-up rows, BGRA.
const pixels = Buffer.alloc(SIZE * SIZE * 4);
for (let y = 0; y < SIZE; y += 1) {
  for (let x = 0; x < SIZE; x += 1) {
    const row = SIZE - 1 - y;
    const i = (row * SIZE + x) * 4;
    if (!insideRounded(x, y)) {
      pixels.writeUInt32LE(0, i);
      continue;
    }
    const t = (x + y) / (2 * (SIZE - 1));
    let r = Math.round(79 + (125 - 79) * t);
    let g = Math.round(140 + (92 - 140) * t);
    let b = Math.round(255 + (255 - 255) * t);
    if (inTriangle(x, y, plane)) {
      r = g = b = 248;
    }
    pixels[i] = b;
    pixels[i + 1] = g;
    pixels[i + 2] = r;
    pixels[i + 3] = 255;
  }
}

const maskRow = SIZE / 8;
const mask = Buffer.alloc(maskRow * SIZE);

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
dir[8] = 0;
dir[9] = 0;
dir.writeUInt16LE(1, 10);
dir.writeUInt16LE(32, 12);
dir.writeUInt32LE(image.length, 14);
dir.writeUInt32LE(22, 18);

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, Buffer.concat([dir, image]));
console.log(out, dir.length + image.length, "bytes");
