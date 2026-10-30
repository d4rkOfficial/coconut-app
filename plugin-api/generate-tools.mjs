#!/usr/bin/env node
/**
 * 从工具箱的打包产物里抽出 WS 工具清单，生成 `tools.d.ts`。
 *
 * 用法（node 18+，无依赖）：
 *   node plugin-api/generate-tools.mjs                # 自动找启动器数据目录里的最新工具箱
 *   node plugin-api/generate-tools.mjs --dir <目录>    # 指定 toolbox-static 目录
 *   node plugin-api/generate-tools.mjs --out <文件>    # 指定输出（默认 plugin-api/tools.d.ts）
 *
 * 工具箱每次更新后重新跑一次，再重新打包启动器，`plugins/api/tools.d.ts` 就会跟着更新。
 */
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..');

function arg(name, fallback) {
  const index = process.argv.indexOf(name);
  return index > 0 && process.argv[index + 1] ? process.argv[index + 1] : fallback;
}

function candidateDirs() {
  const dirs = [];
  const appData = process.env.APPDATA;
  if (appData) {
    dirs.push(path.join(appData, 'com.coconut.launcher', 'toolbox-static'));
    dirs.push(path.join(appData, 'coconut-app', 'toolbox-static'));
  }
  dirs.push(path.join(repo, 'src-tauri', 'toolbox-static'));
  return dirs;
}

function walkJs(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walkJs(full, out);
    else if (entry.name.endsWith('.js')) out.push(full);
  }
  return out;
}

/** 从 start（指向 `{`）开始截一段括号配平的源码。 */
function balancedObject(text, start) {
  let depth = 0;
  let quote = null;
  for (let i = start; i < text.length; i += 1) {
    const ch = text[i];
    if (quote) {
      if (ch === '\\') i += 1;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === '`' || ch === '"' || ch === "'") {
      quote = ch;
      continue;
    }
    if (ch === '{') depth += 1;
    else if (ch === '}') {
      depth -= 1;
      if (depth === 0) return text.slice(start, i + 1);
    }
  }
  return null;
}

/** 打包产物里的参数表偶尔引用同一 chunk 里的常量（枚举清单之类）。
 *  用一个「什么都认、什么都不报错」的 Proxy 兜住未定义标识符。 */
function looseSandbox() {
  const handler = {
    get: (_, key) => {
      if (key === Symbol.toPrimitive) return () => '';
      if (key === Symbol.iterator) return function* empty() {};
      if (key === 'toString' || key === 'valueOf') return () => '';
      if (key === 'length') return 0;
      return proxy;
    },
    apply: () => proxy,
    construct: () => proxy,
    has: () => true,
  };
  const proxy = new Proxy(function loose() {}, handler);
  return proxy;
}

function evaluateLiteral(source) {
  const context = vm.createContext(Object.create(null));
  return vm.runInContext(`(${source})`, context, { timeout: 2000 });
}

function evaluateWithLoose(source) {
  // with + Proxy：未定义标识符不抛 ReferenceError，而是拿到占位值
  const loose = looseSandbox();
  const scope = new Proxy(
    {},
    {
      has: () => true,
      get: (_, key) => (key === Symbol.unscopables ? undefined : loose),
      set: () => true,
    },
  );
  const context = vm.createContext({});
  const fn = vm.runInContext(
    `(function(scope){ with (scope) { return (${source}); } })`,
    context,
    { timeout: 2000 },
  );
  return fn(scope);
}

function tryParse(source) {
  try {
    const value = evaluateLiteral(source);
    if (value && typeof value === 'object') return { value, loosened: false };
  } catch {
    /* 落到宽松模式 */
  }
  try {
    const value = evaluateWithLoose(source);
    if (value && typeof value === 'object') return { value, loosened: true };
  } catch {
    /* 再不行就放弃这个参数表 */
  }
  return { value: null, loosened: true };
}

function collectTools(dir) {
  const tools = new Map();
  const failures = [];
  for (const file of walkJs(dir)) {
    const text = fs.readFileSync(file, 'utf8');
    const re = /[A-Za-z_$][\w$]*\(`([a-z][a-z0-9_]*)`,\{description:/g;
    let match;
    while ((match = re.exec(text))) {
      const name = match[1];
      const braceAt = text.indexOf('{', match.index + name.length + 3);
      const body = balancedObject(text, braceAt);
      if (!body || !body.includes('parameters:')) continue;
      const descStart = body.indexOf('description:') + 'description:'.length;
      const paramAt = body.indexOf('parameters:');
      const descRaw = body.slice(descStart, paramAt).trim().replace(/,$/, '');
      const description = unquote(descRaw);
      const paramBrace = body.indexOf('{', paramAt);
      const paramText = paramBrace >= 0 ? balancedObject(body, paramBrace) : '{}';
      if (!paramText) {
        failures.push([name, 'parameters 截取失败']);
        continue;
      }
      const { value, loosened } = tryParse(paramText);
      if (!value) failures.push([name, 'parameters 解析失败']);
      const previous = tools.get(name);
      const tool = {
        name,
        description: description || (previous ? previous.description : ''),
        params: value && typeof value === 'object' ? value : { type: 'object', properties: {} },
        loosened,
      };
      if (!previous || (previous.loosened && !tool.loosened)) tools.set(name, tool);
    }
  }
  return { tools: tools, failures };
}

/** 把 `` `xxx` `` / `'xxx'` / `"xxx"` 这种字面量取出文本（模板字符串允许换行）。 */
function unquote(raw) {
  const text = raw.trim();
  const quote = text[0];
  if (quote !== '`' && quote !== '"' && quote !== "'") return '';
  let out = '';
  for (let i = 1; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === '\\') {
      const next = text[i + 1];
      out += next === 'n' ? '\n' : next === 't' ? '\t' : next;
      i += 1;
      continue;
    }
    if (ch === quote) break;
    if (ch === '$' && text[i + 1] === '{') break; // 带插值的就不管了
    out += ch;
  }
  return out;
}

/* ───────────────────────── 类型渲染 ───────────────────────── */

function tsLiteral(value) {
  if (typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return null;
}

function renderType(schema, depth = 0) {
  if (!schema || typeof schema !== 'object') return 'unknown';
  if (Array.isArray(schema.enum) && schema.enum.length) {
    const literals = schema.enum.map(tsLiteral).filter((item) => item !== null);
    if (literals.length) return literals.join(' | ');
  }
  const rawType = Array.isArray(schema.type) ? schema.type : [schema.type];
  const types = rawType.filter((item) => typeof item === 'string');
  if (types.length > 1) {
    return types
      .map((item) => renderType({ ...schema, type: item }, depth))
      .filter((item, index, list) => list.indexOf(item) === index)
      .join(' | ');
  }
  const type = types[0];
  if (type === 'string') return 'string';
  if (type === 'number' || type === 'integer') return 'number';
  if (type === 'boolean') return 'boolean';
  if (type === 'null') return 'null';
  if (type === 'array') {
    const inner = renderType(schema.items || {}, depth);
    return inner.includes(' | ') ? `(${inner})[]` : `${inner}[]`;
  }
  if (type === 'object' || schema.properties) {
    return renderObject(schema, depth);
  }
  return 'unknown';
}

function renderObject(schema, depth) {
  const properties = schema.properties && typeof schema.properties === 'object' ? schema.properties : null;
  const additional =
    schema.additionalProperties && typeof schema.additionalProperties === 'object'
      ? renderType(schema.additionalProperties, depth + 1)
      : null;
  if (!properties || !Object.keys(properties).length) {
    return additional ? `Record<string, ${additional}>` : 'Record<string, never>';
  }
  const required = new Set(Array.isArray(schema.required) ? schema.required : []);
  const indent = '  '.repeat(depth + 1);
  const close = '  '.repeat(depth);
  const lines = Object.entries(properties).map(([key, value]) => {
    const optional = required.has(key) ? '' : '?';
    const doc = docComment(value && value.description, indent, 1);
    const safeKey = /^[A-Za-z_$][\w$]*$/.test(key) ? key : JSON.stringify(key);
    return `${doc}${indent}${safeKey}${optional}: ${renderType(value, depth + 1)};`;
  });
  if (additional) lines.push(`${indent}[key: string]: ${additional};`);
  return `{\n${lines.join('\n')}\n${close}}`;
}

function docComment(text, indent, maxLines = 2, maxLength = 220) {
  if (!text) return '';
  const clean = String(text)
    .replace(/\*\//g, '*\\/')
    .replace(/\s+/g, ' ')
    .trim();
  if (!clean) return '';
  const clipped = clean.length > maxLength ? `${clean.slice(0, maxLength - 1)}…` : clean;
  if (maxLines <= 1) return `${indent}/** ${clipped} */\n`;
  return `${indent}/**\n${indent} * ${clipped}\n${indent} */\n`;
}

/* ───────────────────────── 主流程 ───────────────────────── */

let dir = arg('--dir', '');
if (!dir) {
  dir = candidateDirs().find((candidate) => {
    try {
      return fs.existsSync(path.join(candidate, '_app'));
    } catch {
      return false;
    }
  });
}
if (!dir || !fs.existsSync(dir)) {
  console.error('找不到工具箱静态产物目录，请用 --dir 指定 toolbox-static。');
  process.exit(1);
}

const outFile = path.resolve(arg('--out', path.join(here, 'tools.d.ts')));
const { tools, failures } = collectTools(dir);
const list = [...tools.values()].sort((a, b) => a.name.localeCompare(b.name));
if (!list.length) {
  console.error(`在 ${dir} 里没找到任何工具注册（版本太旧或结构变了？）`);
  process.exit(1);
}

const version = (() => {
  try {
    const raw = fs.readFileSync(path.join(dir, '_app', 'version.json'), 'utf8');
    const parsed = JSON.parse(raw);
    return String(parsed.version || parsed.commit || '').slice(0, 40);
  } catch {
    return '';
  }
})();

const head = `// 由 plugin-api/generate-tools.mjs 自动生成，请勿手改。
// 工具箱包体：${
  version ? `version ${version}` : '未标注版本'
}（${new Date().toISOString().slice(0, 10)} 生成）
// 工具箱更新后重新跑：node plugin-api/generate-tools.mjs

/** 每个 WS 工具的入参（来自工具箱自己的 JSON Schema）。 */
export interface CoconutToolParams {
`;

const body = list
  .map((tool) => {
    const summary = (tool.description || '').replace(/\s+/g, ' ').trim();
    const doc = summary
      ? `  /**\n   * ${summary.replace(/\*\//g, '*\\/').slice(0, 300)}\n   */\n`
      : '';
    const key = /^[A-Za-z_$][\w$]*$/.test(tool.name) ? tool.name : JSON.stringify(tool.name);
    return `${doc}  ${key}: ${renderType({ type: 'object', ...tool.params }, 1)};`;
  })
  .join('\n\n');

const tail = `
}

/** 所有可用的工具箱 WS 工具名。 */
export type CoconutToolName = keyof CoconutToolParams;
`;

fs.writeFileSync(outFile, `${head}${body}${tail}`, 'utf8');

console.log(`工具箱目录：${dir}`);
console.log(`工具数量：${list.length}　解析兜底：${list.filter((tool) => tool.loosened).length}`);
if (failures.length) {
  console.log(`未解析参数表：${failures.length}（这些工具的参数退化成 Record<string, never>）`);
  for (const [name, reason] of failures) console.log(`  - ${name}: ${reason}`);
}
console.log(`输出：${outFile}（${fs.statSync(outFile).size} 字节）`);
