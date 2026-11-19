#!/usr/bin/env node
/**
 * 把 assets/coconut/ 下的睁眼 / 闭眼两张立绘，对齐后转成启动页用的 webp。
 *
 * 为什么需要对齐：启动页的眨眼是「两张图叠在一起、用 opacity 切换」。
 * 两张图必须像素级同框（头顶、脚底、身体中线一致），否则闭眼那一帧会看出
 * 「人物跳位」和「两张图同时显示」的重影。
 *
 * 对齐办法：各自扫 alpha 包围盒 → 按包围盒高度做等比缩放 → 让包围盒顶部对齐、
 * 水平居中 → 输出成同样画布尺寸的两张 webp。
 *
 * 用法：node scripts/make-coconut-frames.mjs
 * 依赖：sharp（npm i --no-save sharp）
 */
import { mkdirSync, existsSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const SRC = {
  open: path.join(root, 'assets/coconut/open.png'),
  blink: path.join(root, 'assets/coconut/blink.png'),
};
const OUT = {
  open: path.join(root, 'ui/coconut-open.webp'),
  blink: path.join(root, 'ui/coconut-blink.webp'),
};

const TARGET_H = 1000; // 输出高度（DPR 2 下的显示高度约 500）
const ALPHA_MIN = 8;

async function alphaBox(file) {
  const { data, info } = await sharp(file).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
  const { width, height, channels } = info;
  let left = width;
  let top = height;
  let right = -1;
  let bottom = -1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (data[(y * width + x) * channels + 3] > ALPHA_MIN) {
        if (x < left) left = x;
        if (x > right) right = x;
        if (y < top) top = y;
        if (y > bottom) bottom = y;
      }
    }
  }
  if (right < 0) throw new Error(`${file}: 整张图都是透明的`);
  return { left, top, width: right - left + 1, height: bottom - top + 1 };
}

for (const [key, file] of Object.entries(SRC)) {
  if (!existsSync(file)) throw new Error(`缺少源图：${file}`);
}

const boxes = { open: await alphaBox(SRC.open), blink: await alphaBox(SRC.blink) };
const scaledWidths = {};
for (const key of Object.keys(SRC)) {
  const box = boxes[key];
  scaledWidths[key] = Math.round((box.width * TARGET_H) / box.height);
}
const canvasW = Math.max(scaledWidths.open, scaledWidths.blink);
console.log(`源图包围盒 open=${boxes.open.width}x${boxes.open.height} blink=${boxes.blink.width}x${boxes.blink.height}`);
console.log(`输出画布：${canvasW}x${TARGET_H}`);

for (const key of Object.keys(SRC)) {
  const box = boxes[key];
  const w = scaledWidths[key];
  const x = Math.round((canvasW - w) / 2);

  const trimmed = await sharp(SRC[key])
    .ensureAlpha()
    .extract({ left: box.left, top: box.top, width: box.width, height: box.height })
    .resize({ width: w, height: TARGET_H, fit: 'fill', kernel: 'lanczos3' })
    .png()
    .toBuffer();

  const out = await sharp({
    create: { width: canvasW, height: TARGET_H, channels: 4, background: { r: 0, g: 0, b: 0, alpha: 0 } },
  })
    .composite([{ input: trimmed, left: x, top: 0 }])
    .webp({ quality: 82, alphaQuality: 92, effort: 6 })
    .toBuffer();

  mkdirSync(path.dirname(OUT[key]), { recursive: true });
  const { writeFileSync } = await import('node:fs');
  writeFileSync(OUT[key], out);
  console.log(`${path.relative(root, OUT[key])}: ${(out.length / 1024).toFixed(1)} KB`);
}
