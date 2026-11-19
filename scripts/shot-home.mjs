#!/usr/bin/env node
/**
 * 跑一遍启动页首页截图（亮色 + 暗色各一张），产物放到 dist/。
 *
 * 稳定性处理：
 *   - 等启动器把「启动时检查工具箱」跑完（tail service.log），避免截到「检查中 / 更新中」；
 *   - 连拍 3 帧，挑其中彼此最像的一帧，避开偶发的闭眼帧；
 *   - 不再往四周留白：只输出窗口本身（圆角 + 全透明背景），贴页面直接用。
 *
 * 用法：
 *   npm run shot
 *   npm run shot -- --keep-raw   # 保留 .tmp/shot 下的原始帧
 *
 * 依赖：sharp（npm i --no-save sharp）、已构建好的 dist/coconut-app.exe。
 */
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const EXE = path.join(root, 'dist', 'coconut-app.exe');
const CAPTURE_PS1 = path.join(here, 'capture-window.ps1');
const RAW_DIR = path.join(root, '.tmp', 'shot');
const APP_DIR = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'com.coconut.launcher');
const SETTINGS = path.join(APP_DIR, 'settings.json');
const SERVICE_LOG = path.join(APP_DIR, 'service.log');

const THEMES = [
  { theme: 'light', out: 'preview-light.png' },
  { theme: 'dark', out: 'preview-dark.png' },
];

const keepRaw = process.argv.includes('--keep-raw');
const FRAMES = 3;

let sharp;
try {
  sharp = (await import('sharp')).default;
} catch {
  console.error('缺少 sharp：先跑 `npm i --no-save sharp` 再执行 npm run shot');
  process.exit(1);
}
if (!existsSync(EXE)) {
  console.error(`找不到 ${path.relative(root, EXE)}，先跑一次 \`npm run tauri:build\``);
  process.exit(1);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function psExe() {
  return path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}

function killApp() {
  spawnSync('taskkill', ['/IM', 'coconut-app.exe', '/F'], { stdio: 'ignore' });
}

async function waitForProcessExit(timeoutMs = 4000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const out = spawnSync(psExe(), [
      '-NoProfile',
      '-Command',
      '(Get-Process coconut-app -ErrorAction SilentlyContinue | Measure-Object).Count',
    ]).stdout?.toString().trim();
    if (out === '0') return true;
    await sleep(200);
  }
  return false;
}

/**
 * 等 service.log 出现本次启动的「启动时检查工具箱版本」，
 * 说明工具箱那轮检查已经结束（游戏数据那轮紧随其后，调用方再等一会儿）。
 */
async function waitForLogMarker(fromSize, pattern, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(SERVICE_LOG) && statSync(SERVICE_LOG).size > fromSize) {
      const text = readFileSync(SERVICE_LOG).slice(fromSize).toString('utf8');
      if (pattern.test(text)) return true;
    }
    await sleep(250);
  }
  console.warn('  ! 没等到日志标记，按超时继续');
  return false;
}

/**
 * 等界面「稳定」：连拍两帧、只看左下角的进度 / 状态区域（不含立绘），
 * 两帧一致就说明更新检查已经跑完，不会再截到「检查中 / 更新中」。
 */
async function waitForStableUi(dir, attempts = 20) {
  mkdirSync(dir, { recursive: true });
  for (let i = 0; i < attempts; i++) {
    execFileSync(
      psExe(),
      ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', CAPTURE_PS1, '-OutDir', dir, '-Count', '2', '-IntervalMs', '700', '-SettleMilliseconds', '300'],
      { stdio: 'ignore' },
    );
    const a = path.join(dir, 'frame-1.png');
    const b = path.join(dir, 'frame-2.png');
    if (existsSync(a) && existsSync(b) && (await statusRegionDiff(a, b)) < 0.35) return true;
    await sleep(800);
  }
  console.warn('  ! 界面一直没稳定下来，按最后一次截图继续');
  return false;
}

/** 左下角状态区（进度条 / 状态文案）的平均像素差 */
async function statusRegionDiff(fileA, fileB) {
  const probe = await sharp(fileA).metadata();
  const region = {
    left: 0,
    top: Math.round(probe.height * 0.78),
    width: Math.round(probe.width * 0.62),
    height: Math.round(probe.height * 0.2),
  };
  const a = await sharp(fileA).extract(region).raw().toBuffer({ resolveWithObject: true });
  const b = await sharp(fileB).extract(region).raw().toBuffer({ resolveWithObject: true });
  const step = a.info.channels * 5;
  let sum = 0;
  let count = 0;
  for (let i = 0; i < a.data.length; i += step) {
    sum += Math.abs(a.data[i] - b.data[i]) + Math.abs(a.data[i + 1] - b.data[i + 1]) + Math.abs(a.data[i + 2] - b.data[i + 2]);
    count += 3;
  }
  return count ? sum / count : 0;
}

/** 从连拍的几帧里挑一帧：选「最像别人」的那帧，避开闭眼那一帧 */
async function pickStableFrame(files) {
  if (files.length === 1) return files[0];
  const raws = [];
  for (const file of files) {
    raws.push(await sharp(file).raw().toBuffer({ resolveWithObject: true }));
  }
  const diff = (a, b) => {
    const step = a.info.channels * 7; // 抽样，快一些
    let sum = 0;
    for (let i = 0; i < a.data.length; i += step) {
      sum += Math.abs(a.data[i] - b.data[i]) + Math.abs(a.data[i + 1] - b.data[i + 1]) + Math.abs(a.data[i + 2] - b.data[i + 2]);
    }
    return sum;
  };
  let bestIndex = 0;
  let bestScore = Infinity;
  for (let i = 0; i < raws.length; i++) {
    let score = 0;
    for (let j = 0; j < raws.length; j++) {
      if (i !== j) score += diff(raws[i], raws[j]);
    }
    if (score < bestScore) {
      bestScore = score;
      bestIndex = i;
    }
  }
  return files[bestIndex];
}

/** 窗口截图 -> 圆角、透明底、无留白的成品 */
async function composeWindow(rawPath, outPath) {
  const png = await sharp(rawPath).png().toBuffer();
  const meta = await sharp(png).metadata();
  const radius = 16;
  const rounded = await sharp(png)
    .composite([
      {
        input: Buffer.from(
          `<svg xmlns="http://www.w3.org/2000/svg" width="${meta.width}" height="${meta.height}"><rect x="0" y="0" width="${meta.width}" height="${meta.height}" rx="${radius}" ry="${radius}" fill="#ffffff"/></svg>`,
        ),
        blend: 'dest-in',
      },
    ])
    .png({ compressionLevel: 9 })
    .toFile(outPath);
  console.log(`  → ${path.relative(root, outPath)} (${rounded.width}x${rounded.height}, 透明底圆角)`);
}

const settingsBackup = existsSync(SETTINGS) ? readFileSync(SETTINGS) : null;
let code = 0;

try {
  mkdirSync(RAW_DIR, { recursive: true });
  for (const item of THEMES) {
    killApp();
    await waitForProcessExit();

    mkdirSync(APP_DIR, { recursive: true });
    let current = {};
    try {
      current = JSON.parse(readFileSync(SETTINGS, 'utf8'));
    } catch {}
    writeFileSync(SETTINGS, `${JSON.stringify({ ...current, theme: item.theme }, null, 2)}\n`, 'utf8');

    const logOffset = existsSync(SERVICE_LOG) ? statSync(SERVICE_LOG).size : 0;

    console.log(`[${item.theme}] 启动启动器…`);
    const child = spawn(EXE, [], { detached: true, stdio: 'ignore' });
    child.on('error', (error) => console.error(`  启动失败：${error.message}`));
    child.unref();

    await waitForLogMarker(logOffset, /启动时检查工具箱版本/);
    // 工具检查完 350ms 后还会跑一轮游戏数据检查，结果文案再挂 3 秒，等它彻底静止
    await sleep(5000);
    await waitForStableUi(path.join(RAW_DIR, `${item.theme}-probe`));

    // 稳定后连拍几帧，避开偶发的闭眼帧
    const settledDir = path.join(RAW_DIR, `${item.theme}-settled`);
    mkdirSync(settledDir, { recursive: true });
    execFileSync(
      psExe(),
      [
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        CAPTURE_PS1,
        '-OutDir',
        settledDir,
        '-Count',
        String(FRAMES),
        '-IntervalMs',
        '190',
        '-SettleMilliseconds',
        '500',
      ],
      { stdio: 'inherit' },
    );

    const frames = Array.from({ length: FRAMES }, (_, i) => path.join(settledDir, `frame-${i + 1}.png`)).filter((f) =>
      existsSync(f),
    );
    const chosen = await pickStableFrame(frames);
    console.log(`[${item.theme}] 选用 ${path.basename(chosen)}，合成成品…`);
    await composeWindow(chosen, path.join(root, 'dist', item.out));

    killApp();
    await waitForProcessExit();
  }
} catch (error) {
  console.error(error.message || error);
  code = 1;
} finally {
  killApp();
  if (settingsBackup) writeFileSync(SETTINGS, settingsBackup);
  if (!keepRaw) rmSync(RAW_DIR, { recursive: true, force: true });
}

process.exit(code);
