#!/usr/bin/env node
/**
 * 内置脚本打包：把 `builtin/<名字>/index.js` 用 esbuild 压成单文件，
 * 产物写到 `builtin/out/<名字>.js`，再由 Rust（`include_str!`）编进 exe。
 *
 * 用法：
 *   npm run builtin            # 打包全部
 *   node builtin/build.mjs 免费模型目录名     # 只打一个
 *
 * 注意：
 * - 插件运行时是 QuickJS，没有 import/export，所以打包成 IIFE；
 * - 打包后钩子不在全局作用域里，**每个入口文件末尾要自己挂出去**：
 *     globalThis.onHello = onHello;
 * - `// @name` 这类头部注释会被 minify 掉，这里单独抽出来用 banner 放回去，
 *   否则插件元数据（名字 / 权限）会全部丢失。
 */
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(here, 'out');
const only = process.argv[2] || '';

function entryDirs() {
  return fs
    .readdirSync(here, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter((name) => name !== 'out' && name !== 'node_modules')
    // `*.translate` / `*.bak` / 以 . 开头的是留档目录，不参与打包
    .filter((name) => !name.endsWith('.translate') && !name.endsWith('.bak') && !name.startsWith('.'))
    .filter((name) => !only || name === only)
    .filter((name) => fs.existsSync(path.join(here, name, 'index.js')));
}

/** 抽出文件开头连续的 `//` 注释块里的 `@xxx` 行（保留顺序）。 */
function headerOf(source) {
  const lines = source.replace(/\r\n/g, '\n').split('\n');
  const header = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) {
      if (header.length) header.push('//');
      continue;
    }
    if (!trimmed.startsWith('//')) break;
    if (trimmed.startsWith('// @')) header.push(trimmed);
  }
  return header.join('\n');
}

async function buildOne(name) {
  const dir = path.join(here, name);
  const entry = path.join(dir, 'index.js');
  const source = fs.readFileSync(entry, 'utf8');
  const header = headerOf(source);
  if (!/^\/\/ @name/m.test(header)) {
    throw new Error(`${name}/index.js 缺少 // @name 头部元数据`);
  }
  const outfile = path.join(outDir, `${name}.js`);
  const result = await esbuild.build({
    entryPoints: [entry],
    outfile,
    bundle: true,
    format: 'iife',
    platform: 'neutral',
    target: 'es2020',
    minify: true,
    legalComments: 'none',
    banner: { js: `${header}\n` },
    logLevel: 'warning',
    metafile: true,
  });
  const size = fs.statSync(outfile).size;
  const inputs = Object.keys(result.metafile.inputs).length;
  console.log(
    `  ${name.padEnd(16)} ${String(inputs).padStart(2)} 个文件 → out/${name}.js  ${(
      size / 1024
    ).toFixed(1)} KB`,
  );
}

fs.mkdirSync(outDir, { recursive: true });
const names = entryDirs();
if (!names.length) {
  console.error(only ? `找不到内置脚本：${only}` : 'builtin/ 下没有可打包的脚本');
  process.exit(1);
}
console.log(`打包 ${names.length} 个内置脚本：`);
for (const name of names) await buildOne(name);
console.log(`完成，产物在 ${path.relative(process.cwd(), outDir)}`);
