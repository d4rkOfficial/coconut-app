// @name        免费模型
// @version     1.0.0
// @description 把启动器免费模型代理里的模型同步成工具箱的 AI 配置（内置插件，可停用）
// @permissions toolbox, dom, storage, network
// @mount       none
//
// 这就是「免费模型」这套功能的全部逻辑，用的是和用户插件完全一样的 Coconut API：
//   - 模型列表来自启动器自己的 /v1/models（那边一天只向上游打一次，结果带缓存）；
//   - 写配置走 Coconut.callTool('manage_ai_profile')；
//   - 判断用户有没有关 AI 助手要读工具箱 IndexedDB，走 Coconut.runJs；
//   - 「第一次用」用 Coconut.storage 记，避免把工具箱首次进入写下的 enabled=false 当用户选择。

import {
  openAssistant as openAssistantPage,
  readAiConfig as readAiConfigPage,
  readAiPrefs as readAiPrefsPage,
} from './free-models.dom.js';

const BASE_URL = 'http://127.0.0.1:47821/v1';
const PUBLIC_KEY = 'public';
const MANAGED_PREFIX = '\u200B';
const MANAGED_LABEL = '\u200BOC_FREE\u200C';
const LEGACY_PREFIX = '椰果免费 · ';

function log(...args) {
  Coconut.log.apply(null, args.map((v) => (typeof v === 'string' ? v : JSON.stringify(v))));
}



// 页面侧的小脚本都是普通函数（page.js），压缩之后再序列化进 runJs，
// 这里只负责把函数源码包成一段可执行的表达式。
const pageCall = (fn) => `return (${String(fn).trim()})();`;

/** 读工具箱的 ai-gen-prefs：{ found, enabled } / null（读不到）。 */
async function readAiPrefs() {
  const code = pageCall(readAiPrefsPage);
  for (let attempt = 0; attempt < 6; attempt++) {
    try {
      const parsed = JSON.parse(Coconut.runJs(code));
      if (parsed && parsed.error) {
        await Coconut.sleep(500);
        continue;
      }
      return parsed;
    } catch (error) {
      // 页面还在启动时 runJs 会报「工具箱页面还没打开」：等一下重试，别急着放弃
      if (attempt < 2) log('读取 AI 助手开关失败：' + error);
      await Coconut.sleep(700);
    }
  }
  return null;
}

/**
 * 工具箱是异步把 ai-config 从 IndexedDB 读进内存的：页面刚起来时
 * get_ai_profiles 可能还返回空列表（默认配置之类），这时候直接 add 会写出重复配置。
 * 所以先看 IDB 里到底有多少条，再等内存里的列表追上。
 */
async function waitForConfig() {
  let stored = null;
  try {
    stored = JSON.parse(Coconut.runJs(pageCall(readAiConfigPage)));
  } catch (error) {
    return false;
  }
  if (!stored || !Array.isArray(stored.profiles) || !stored.profiles.length) {
    // 真的没有历史配置：等一个节拍让工具箱初始化完
    await Coconut.sleep(1000);
    return true;
  }
  for (let attempt = 0; attempt < 16; attempt++) {
    const current = Coconut.callTool('get_ai_profiles', {}) || {};
    const ids = new Set((current.profiles || []).map((profile) => String(profile.id)));
    if (stored.profiles.every((profile) => ids.has(String(profile.id)))) return true;
    await Coconut.sleep(250);
  }
  log('等工具箱加载配置超时，按当前列表继续');
  return true;
}

/** 兜底去重：同一 model 只留第一条托管配置。 */
function dedupeManaged() {
  const state = Coconut.callTool('get_ai_profiles', {}) || {};
  const seen = new Set();
  let removed = 0;
  for (const profile of state.profiles || []) {
    if (!isManaged(profile)) continue;
    const model = String(profile.model || '');
    if (!seen.has(model)) {
      seen.add(model);
      continue;
    }
    if (deleteProfile(profile)) removed += 1;
  }
  return removed;
}

function isManaged(profile) {
  return String(profile.label || '').startsWith(MANAGED_PREFIX);
}

function isEmptyProfile(profile) {
  const empty = (value) => String(value || '').trim() === '';
  return empty(profile.baseUrl) && empty(profile.model) && profile.apiKeySet !== true;
}

async function fetchModels() {
  for (let attempt = 0; attempt < 4; attempt++) {
    // Coconut.fetch 返回 { status, ok, headers, json, body }，模型列表在 json.data
    const payload = Coconut.fetch('http://127.0.0.1:47821/v1/models', {});
    const list = (payload && payload.json && payload.json.data) || [];
    const ids = list.map((item) => String(item.id)).filter(Boolean);
    if (ids.length) return ids;
    // 启动器刚起来时模型列表还在拉，等一下再问一次
    await Coconut.sleep(1200);
  }
  return [];
}

function profileFields(model) {
  return {
    baseUrl: BASE_URL,
    model,
    // 真实 Key 由启动器代理兜底：这里的 public 不是合法 Key，代理会换成用户存的那把。
    apiKey: PUBLIC_KEY,
    maxToolRounds: 48,
    directRequest: false,
  };
}

function deleteProfile(profile) {
  try {
    Coconut.callTool('manage_ai_profile', { action: 'delete', id: profile.id });
    return true;
  } catch (error) {
    // 工具箱要求至少保留一条：把它改回空白「默认配置」，列表里也就没有免费模型了
    try {
      Coconut.callTool('manage_ai_profile', {
        action: 'update',
        id: profile.id,
        label: '默认配置',
        baseUrl: '',
        model: '',
        apiKey: '',
      });
    } catch (inner) {
      log('清理配置失败：' + inner);
    }
    return false;
  }
}

/** 把当前模型列表同步进工具箱。 */
function syncModels(models) {
  const state = Coconut.callTool('get_ai_profiles', {}) || {};
  const profiles = state.profiles || [];
  const managed = profiles.filter(isManaged);
  let added = 0;
  let updated = 0;

  for (const model of models) {
    const index = managed.findIndex((profile) => String(profile.model || '') === model);
    if (index >= 0) {
      const profile = managed.splice(index, 1)[0];
      const args = Object.assign(
        { action: 'update', id: profile.id, label: MANAGED_LABEL },
        profileFields(model)
      );
      Coconut.callTool('manage_ai_profile', args);
      updated += 1;
    } else {
      const args = Object.assign({ action: 'add', label: MANAGED_LABEL }, profileFields(model));
      Coconut.callTool('manage_ai_profile', args);
      added += 1;
    }
  }

  let removed = 0;
  for (const profile of managed) {
    if (deleteProfile(profile)) removed += 1;
  }
  for (const profile of profiles) {
    const label = String(profile.label || '');
    if (label.startsWith(LEGACY_PREFIX)) {
      if (deleteProfile(profile)) removed += 1;
    } else if ((profile.id === 'default' || label === '默认配置') && isEmptyProfile(profile)) {
      if (deleteProfile(profile)) removed += 1;
    }
  }
  return { added, updated, removed };
}

async function openAssistantPanel() {
  const code = pageCall(openAssistantPage);
  for (let attempt = 0; attempt < 5; attempt++) {
    try {
      const result = String(Coconut.runJs(code));
      if (result === 'already' || result === 'opened') return true;
    } catch (error) {
      log('打开 AI 悬浮窗失败：' + error);
      return false;
    }
    await Coconut.sleep(400);
  }
  return false;
}

function setSetting(key, value) {
  try {
    Coconut.callTool('set_setting', { key, value });
  } catch (error) {
    log('设置 ' + key + ' 失败：' + error);
  }
}

async function runSync() {
  // 「写过一次」要和「这个工具箱确实有偏好」一起看：
  // 浏览器数据被清掉（新 profile）时 prefs 不存在，这时应重新按首次使用处理。
  const storedSeeded = Coconut.storage.get('seeded', false) === true;
  const prefs = await readAiPrefs();
  if (prefs === null) {
    // 页面刚起来 / 工具箱还没挂好：状态未知就别动用户的配置，过几秒重试
    scheduleRetry(3000, '读不到 AI 助手开关');
    return;
  }
  if (!(await waitForConfig())) {
    log('工具箱配置还没准备好，跳过本次同步');
    return;
  }
  const seeded = storedSeeded && prefs.found === true;
  const aiOff = prefs.found === true && prefs.enabled === false && seeded;

  if (aiOff) {
    const state = Coconut.callTool('get_ai_profiles', {}) || {};
    let removed = 0;
    for (const profile of state.profiles || []) {
      if (isManaged(profile) && deleteProfile(profile)) removed += 1;
    }
    log('用户关掉了 AI 助手，已清理 ' + removed + ' 条免费模型配置');
    return;
  }

  let models = [];
  try {
    models = await fetchModels();
  } catch (error) {
    log('拿免费模型列表失败：' + error);
    return;
  }
  if (!models.length) {
    log('免费模型列表为空，跳过同步');
    return;
  }

  retryCount = 0;
  const result = syncModels(models);
  const deduped = dedupeManaged();
  setSetting('ai_enabled', true);
  if (!seeded) {
    Coconut.storage.set('seeded', true);
    log('首次使用：写入 ' + models.length + ' 个免费模型并打开 AI 助手');
  } else {
    log(
      '同步完成：新增 ' + result.added + '，更新 ' + result.updated + '，清理 ' +
        (result.removed + deduped)
    );
  }
  if (prefs.found !== true || prefs.enabled === true) {
    await openAssistantPanel();
  }
}

async function onStart() {
  log('插件已加载（' + Coconut.plugin.version + '）');
}

let retryTimer = null;
let retryCount = 0;
const MAX_RETRIES = 6;

/** 页面还在启动 / 正在刷新时，过几秒自己再试一次（不依赖新的 hello）。 */
function scheduleRetry(delay, reason) {
  if (retryTimer) return;
  if (retryCount >= MAX_RETRIES) {
    log(reason + '：已重试 ' + retryCount + ' 次，等下一次页面就绪');
    return;
  }
  retryCount += 1;
  log(reason + '：' + delay / 1000 + ' 秒后重试（第 ' + retryCount + ' 次）');
  retryTimer = Coconut.setTimeout(async () => {
    retryTimer = null;
    await safeSync('retry');
  }, delay);
}

function isPageMissing(error) {
  return /页面还没打开|WS 连接已关闭|WS 发送失败/.test(String(error));
}

async function safeSync(reason) {
  try {
    await runSync();
  } catch (error) {
    if (isPageMissing(error)) {
      scheduleRetry(2500, reason + '：工具箱页面还没连上');
      return;
    }
    throw error;
  }
}

async function onHello() {
  await safeSync('onHello');
}

async function onEvent(event) {
  if (!event || event.type !== 'resync') return;
  await safeSync('resync');
}

async function onStop() {
  log('插件停止');
}

// —— 交给引擎 ——
// esbuild 会把这整份代码包进 IIFE，函数声明不再落在全局作用域；
// 插件宿主是在 globalThis 上找钩子的，所以这里显式挂出去。
globalThis.onStart = onStart;
globalThis.onHello = onHello;
globalThis.onEvent = onEvent;
globalThis.onStop = onStop;