# 椰果工具箱启动器插件开发指南

本文件既能给开发者看，也能给 AI Agent 读：前半是可直接运行的示例，后半是按当前启动器
实际行为写的规范。示例复制到 `plugins\` 目录即可验证。

> **工程化开发请用插件开发 SDK 仓库**：
> [`coconut-app-plugin-sdk`](https://github.com/d4rkOfficial/coconut-app-plugin-sdk)
>
> 它把本文件的规则落成了可执行环境：模块化源码（`page/*.dom.js` 放注入页面的脚本）、
> esbuild 单文件打包（压缩 + 保留 `@name` 头部）、`deno task dev <id>` 热部署、
> 静态自查（权限 / 钩子 / 禁用 API / `.dom.js` 自包含）和假宿主冒烟测试。
> 本文件继续作为 **API 规范原文**维护，SDK 的 `docs/plugin-api.md` 就是它的一份拷贝。

---

# 第一部分 · 示例

下面每个示例都是**完整可运行**的单个 `.js` 文件。存到：

```text
%APPDATA%\com.coconut.launcher\plugins\
```

文件名就是插件 id（`eye-care.js` → id `eye-care`），编码必须是 **UTF-8 无 BOM**。
拖进目录后最多 3 秒自动加载。

## 示例 1 · 最小骨架（不碰页面）

```js
// @name        心跳
// @version     1.0.0
// @description 只在启动器里写日志，不依赖工具箱页面

async function onStart() {
  Coconut.log('心跳插件启动：', Coconut.plugin.id);
}

async function onStop() {
  Coconut.log('心跳插件退出');
}
```

`onStart` 在工具箱页面打开之前就会跑，所以这里**不能**调 `Coconut.runJs`。

## 示例 2 · 注入 CSS + 右下角浮层

```js
// @name        护眼滤镜
// @version     1.0.0
// @description 给工具箱加一层暖色滤镜，并在右下角显示一个小浮层
// @permissions dom, ui
// @mount       overlay

const CSS = 'html { filter: sepia(0.22) saturate(1.04) !important; }';

let cardId = 0;

async function onHello() {
  Coconut.ui.injectCss('eye-care', CSS);
  cardId = Coconut.ui.overlay({
    html: '<div class="card">护眼滤镜已开启</div>',
    css: '.card { position: absolute; right: 20px; bottom: 20px; padding: 10px 14px;'
      + ' border-radius: 12px; background: rgba(28,24,18,.86); color: #f3e6cf;'
      + ' pointer-events: auto; }',
  });
  Coconut.log('护眼滤镜已挂到工具箱页面，浮层 id =', cardId);

  await Coconut.sleep(2500);
  Coconut.ui.hide(cardId); // 也可以之后再 show
}

async function onStop() {
  Coconut.ui.injectCss('eye-care', ''); // 传空字符串等于撤掉
  if (cardId) Coconut.ui.remove(cardId);
}
```

`ui.overlay` 只负责「一段 html + 一段 css」，铺满视口的容器和 **z-index 都由引擎管**
（后注册的盖住先注册的），浮层本身不拦鼠标，要交互的元素自己写 `pointer-events: auto`。
`ui.injectCss` 是页面级 CSS（影响整页），和浮层无关。

## 示例 3 · 只调工具箱工具（不碰 DOM）

```js
// @name        主题跟随
// @version     1.0.0
// @description 把工具箱的主题切到深色
// @permissions toolbox

async function onHello() {
  const applied = Coconut.callTool('set_setting', { key: 'theme_mode', value: 'dark' });
  Coconut.log('工具箱主题已切换：', JSON.stringify(applied));
}
```

工具名和参数跟工具箱内置 AI 用的是同一套，先试一个 `{}` 看返回结构，再照 `tools.d.ts` 补参数。

## 示例 4 · 本地缓存 + 外部请求

```js
// @name        版本公告
// @version     1.0.0
// @description 每小时拉一次上游最新版本，失败时退回本地缓存
// @permissions dom, ui, network, storage

const TTL = 60 * 60 * 1000;
const FEED = 'https://api.github.com/repos/owner/repo/releases/latest';

let noticeId = 0;

async function load(force) {
  const cached = Coconut.storage.get('feed', null);
  if (!force && cached && Date.now() - cached.at < TTL) return cached;

  const res = Coconut.fetch(FEED, {
    headers: { accept: 'application/vnd.github+json', 'user-agent': 'coconut-plugin' },
  });
  if (!res.ok) throw new Error('HTTP ' + res.status);

  const next = {
    at: Date.now(),
    tag: String((res.json && res.json.tag_name) || '未知'),
  };
  Coconut.storage.set('feed', next);
  return next;
}

async function onHello() {
  try {
    const feed = await load(false);
    noticeId = Coconut.ui.overlay({
      html: '<div class="notice">最新版本：<b>' + feed.tag + '</b></div>',
      css: '.notice { position: absolute; left: 20px; bottom: 20px; padding: 8px 12px;'
        + ' border-radius: 10px; background: rgba(20,20,22,.86); color: #eee;'
        + ' pointer-events: auto; }',
    });
  } catch (error) {
    // 拿不到就用旧缓存，别让钩子抛错（连抛 3 次会被自动停用）
    Coconut.log('拉取公告失败：', String(error));
  }
}
```

`Coconut.fetch` 走启动器进程的 reqwest，**不受页面 CORS 限制**，返回
`{ status, ok, headers, body, json }`，`json` 是自动 parse 好的对象（失败为 `null`）。

## 示例 5 · 等页面就绪再干活（重试模板）

```js
// @name        页面标题播报
// @version     1.0.0
// @description 在 onStart 里轮询，直到工具箱页面可用
// @permissions dom

async function waitForPage() {
  for (let attempt = 0; attempt < 12; attempt++) {
    try {
      const title = Coconut.runJs('return document.title');
      if (title) return title;
    } catch (error) {
      // 页面还没打开时 runJs 会抛「工具箱页面还没打开」
    }
    await Coconut.sleep(500);
  }
  return null;
}

async function onStart() {
  const title = await waitForPage();
  Coconut.log('工具箱页面标题：', title || '（6 秒内没就绪）');
}
```

每个钩子有 **10 秒**上限，轮询总时长要留余量（这里 12 × 500ms ≈ 6 秒）。

## 示例 6 · 响应 resync 事件 + 收尾

```js
// @name        配置速览
// @version     1.0.0
// @description 页面就绪时和 resync 时各读一次 AI 配置并写日志
// @permissions toolbox

async function refresh(reason) {
  const state = Coconut.callTool('get_ai_profiles', {}) || {};
  const profiles = state.profiles || [];
  Coconut.log('[' + reason + '] AI 配置 ' + profiles.length + ' 条，当前 ' +
    String(state.activeId || '—'));
}

async function onHello() {
  await refresh('hello');
}

async function onEvent(event) {
  if (event.type === 'resync') await refresh('resync');
}
```

`resync` 由启动器在模型列表就绪、主题切换等时机发出，插件只管响应。

## 示例 7 · 观察原始 WS 帧（仅开发时）

```js
// @name        帧观察器
// @version     1.0.0
// @description 打印启动器 ↔ 工具箱的每一帧，日志写在文件里
// @permissions debug

async function onEvent(event) {
  if (event.type !== 'frame') return;
  const text = JSON.stringify(event.frame);
  Coconut.log(event.direction, text.length > 200 ? text.slice(0, 200) + '…' : text);
}
```

`debug` 会收到**全部** WS 帧（可能包含用户的操作参数），正式发布前请去掉这个权限。

---

# 第二部分 · 文档正文（新版）

## 1. 插件是什么

启动器（Tauri 外壳）为静态的椰果工具箱起本地服务，并用内置的 WebSocket 接管通道
「遥控」工具箱页面。插件是跑在**启动器进程内、独立线程上的 QuickJS（rquickjs 0.8）脚本**，
通过 `Coconut` 这一套 API 去读页面、调工具箱工具、发 HTTP、画浮层、写日志。

因此：

- 插件不是浏览器扩展，也不加载到页面里，**碰不到工具箱的 JS 上下文**，只能通过 API 间接操作；
- 插件之间互相隔离（各自线程、各自存储），一个死循环不会拖死启动器；
- 只有一个全局对象叫 `Coconut`（旧文档里的 `ctx` / `Oc*` 命名已全部作废）。

## 2. 文件形态与元数据

| 规则 | 说明 |
| --- | --- |
| 单个 `.js` 文件 | 不是模块，没有 `import` / `export` / `require` |
| 元数据写在文件最前面的注释里 | 只认第一行代码之前的 `// @key value` |
| UTF-8 **无 BOM** | 带 BOM 的文件会读不干净（启动器会剥掉 BOM，但别依赖这个兜底） |
| 钩子必须是全局函数声明 | `async function onHello() {}` ✅；`const onHello = async () => {}` ❌ |
| 文件名 = 插件 id | 以 `_` 开头的文件会被跳过（放模板用） |
| 修改文件 | 最多 3 秒自动重新加载 |

元数据键（**没有 `@apiVersion`**，版本号已经废除了）：

| 键 | 必填 | 取值 |
| --- | --- | --- |
| `@name` | 建议 | 展示名，不写就用文件名 |
| `@version` | 建议 | 任意字符串，卡片里显示 `v…` |
| `@description` | 可选 | 一句话说明，显示在插件卡片里 |
| `@permissions` | 按需 | 逗号分隔权限名，见下一节 |
| `@mount` | 可选 | `none`（默认）/ `overlay`（声明要用浮层） |

## 3. 权限

| 权限 | 解锁 | 没申请时 |
| --- | --- | --- |
| `dom` | `Coconut.runJs` | 抛「插件缺少 dom 权限」 |
| `toolbox` | `Coconut.callTool` | 抛「插件缺少 toolbox 权限」 |
| `network` | `Coconut.fetch` | 抛「插件缺少 network 权限」 |
| `storage` | `Coconut.storage.*` | 抛「插件缺少 storage 权限」 |
| `ui` | `Coconut.ui.overlay / visible / remove / injectCss` | 抛「插件缺少 ui 权限」（`overlay`/`injectCss` 还要 `dom`） |
| `debug` | `onEvent` 里的原始 WS 帧 | 收不到 `frame` 事件 |

`Coconut.log` / `console.*` / 定时器**不需要**权限。

权限是 Rust 侧硬校验的，插件卡片会把声明的权限原样展示给用户——只申请真正用得到的。

> 旧文档里的 `notify` 权限和 `Coconut.notify(text)` **已经删除**，没有替代接口：
> 要提示用户就画浮层（`Coconut.ui.overlay`）或写日志。

## 4. 生命周期与时限

| 钩子 | 时机 | 参数 |
| --- | --- | --- |
| `onStart()` | 插件线程起来（页面可能还没打开） | 无 |
| `onHello(info)` | 工具箱页面就绪 | `{ session: string, tools: string[] }` |
| `onEvent(event)` | `resync`；有 `debug` 权限时还有 `frame` | `CoconutEvent` |
| `onStop()` | 停用 / 重载 / 退出 | 无 |

- 四个钩子都可以是 `async function`，`await` 宿主 API 没问题；
- **单个钩子超过 10 秒**会被 QuickJS 中断，记一次错误；
- **连续 3 次抛错自动停用**，插件卡片标红并显示最后一次错误，用户点「启用」恢复；
- `onHello` 在每次页面就绪时都会触发（页面刷新、WS 重连），**必须可重复执行**；
- 启用状态落在 `plugins/state.json`，插件存储落在 `plugins/storage/<id>.json`。

## 5. `Coconut` API 速查

### `Coconut.plugin`
`{ id, name, version, mount, permissions }`，只读。

### `Coconut.runJs(code)` → `string`（`dom`）

在工具箱页面里执行 JS。**传的是函数体**——工具箱会包成
`return (async () => { <你的代码> })()`，所以自己写 `return`：

```js
const count = Number(Coconut.runJs('return document.querySelectorAll(".card").length'));
const prefs = JSON.parse(Coconut.runJs(`
  const record = await new Promise((resolve) => {
    const req = indexedDB.open('wuwa-v1', 1);
    req.onsuccess = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('cache')) return resolve(null);
      const get = db.transaction('cache', 'readonly').objectStore('cache').get('ai-gen-prefs');
      get.onsuccess = () => resolve(get.result ? get.result.data : null);
    };
    req.onerror = () => resolve(null);
  });
  return JSON.stringify(record);
`));
```

返回值永远是字符串（`String(...)`），要对象就自己 `JSON.stringify` / `JSON.parse`；
页面里抛错会变成 JS 异常。

### `Coconut.callTool(name, args?)` → 工具返回值（`toolbox`）

```js
const state = Coconut.callTool('get_ai_profiles', {});
Coconut.callTool('manage_ai_profile', {
  action: 'add',
  label: '我的配置',
  baseUrl: 'http://127.0.0.1:47821/v1',
  model: 'deepseek-v4.1-flash',
});
Coconut.callTool('set_setting', { key: 'theme_mode', value: 'dark' });
```

工具报错会抛成 JS 异常（→ 钩子抛错 → 错误计数）。

工具名和入参由 `plugin-api/tools.d.ts` 提供类型（143 个工具，从工具箱包体自动生成）：
写 `Coconut.callTool(` 时编辑器会补全工具名，参数写错 / 少传必填项会直接报错。
返回类型是 `unknown`（工具箱没暴露返回结构），要字段自己断言：

```js
const state = /** @type {{ activeId?: string, profiles: { id: string, model: string }[] }} */ (
  Coconut.callTool('get_ai_profiles', {})
);
```

工具箱刚更新、`tools.d.ts` 还没重新生成时，用
`Coconut.callTool(/** @type {any} */ ('新工具'), {})` 绕过名字检查。

### `Coconut.fetch(url, options?)` → `{ status, ok, headers, body, json }`（`network`）

`options`：`{ method, headers, body, json }`。不走页面，因此没有 CORS；域名不限。
**没有超时参数**，长时间卡住的请求会算进 10 秒钩子预算里。

### `Coconut.storage`（`storage`）

`get(key, fallback) / set(key, value) / delete(key) / all()`；值必须 JSON 可序列化。

### `Coconut.ui`（`ui`，`overlay` / `injectCss` 另外需要 `dom`）

浮层只由引擎管位置和层级，插件给「初始 html + 初始 css」，拿到 id 之后自己管显隐：

```js
const id = Coconut.ui.overlay({
  html: '<div class="card">你好</div>',
  css: '.card { position: absolute; right: 20px; bottom: 20px; pointer-events: auto; }',
});
Coconut.ui.hide(id);      // 也可以 .show(id) / .visible(id, false)
Coconut.ui.remove(id);    // 删掉，再操作这个 id 会返回 false
```

- 浮层容器铺满视口、**不拦截鼠标**；要让内容可点，自己写 `pointer-events: auto`；
- **z-index 由引擎编排**：后注册的盖住先注册的，插件不用也不能指定层级；
- `overlay` 返回的是数字 id（从 1 递增），同一个插件注册多个浮层各拿一个 id；
- `visible` / `show` / `hide` / `remove` 都返回 boolean（没找到这个 id 时为 `false`）；
- `injectCss(id, css)` 是**页面级**样式，跟浮层无关，同名再调一次是替换，传 `''` 撤销。

### `Coconut.log(...args)` / `console.*`

两者都写进插件日志文件。`Coconut.log` 用 `String()` 拼参数，对象会变成
`[object Object]`——要结构化请自己 `JSON.stringify`；`console.*` 会帮非字符串做
`JSON.stringify`。

### `Coconut.launcher` / `Coconut.src`（只读，不需要权限）

```js
Coconut.launcher.lang;           // 'zh-Hans' | 'zh-Hant' | 'ja' | 'ko' | 'en'
Coconut.launcher.toolboxVersion; // 工具箱包体版本
Coconut.launcher.gameVersion;    // 游戏数据版本

Coconut.src.toolbox.dir;         // 工具箱包体目录（绝对路径）
Coconut.src.toolbox.url;         // http://127.0.0.1:47821/
Coconut.src.static.dir;          // 启动器本地资源目录（游戏图标等）
Coconut.src.static.url;          // http://127.0.0.1:47821/icons/
```

- `toolbox` = 椰果工具箱包体目录；`static` = 启动器本地资源目录（本地化游戏图标）；
- 每个根只给 **`.dir`（本地路径）** 和 **`.url`（启动器本地服务地址）**；
- 要给 `fetch` 或 `<img src>` 用就用 `.url` 拼相对路径（浏览器拿不到本地磁盘路径）：

```js
// 读工具箱自己的静态文件（包体被启动器挂在根路径上）
const html = Coconut.fetch(Coconut.src.toolbox.url + '_app/version.json').body;

// 直接把本地图标当图片地址用
const icon = Coconut.src.static.url + '01186e89b60e0d4d.webp';
Coconut.ui.overlay({
  html: '<img class="logo" src="' + icon + '">',
  css: '.logo { position: absolute; right: 20px; bottom: 20px; width: 72px; pointer-events: auto; }',
});
```

### `Coconut.sleep(ms)` / 定时器

`await Coconut.sleep(500)` 就是「等半秒」，钩子会等它跑完（也计入 10 秒预算）。

`Coconut.setTimeout / setInterval / clearTimeout / clearInterval` 跑在插件自己的
事件循环上，回调**不带参数**。

## 6. 运行时边界（实测）

- 引擎 QuickJS，支持到 ES2020：`async/await`、`?.`、`??`、`??=`、展开、`Map/Set/WeakMap`、
  `Proxy/Reflect`、`BigInt`、`ArrayBuffer/Uint8Array`、`Date`、`JSON`、`Promise`、
  `queueMicrotask`、`setTimeout` 系列。
- **没有**：`fetch`、`URL/URLSearchParams`、`TextEncoder/Decoder`、`atob/btoa`、
  `structuredClone`、`crypto`、`Buffer`、`process`、`require`、`import`。
  发请求用 `Coconut.fetch`，base64 / URL 解析自己写。
- 没有文件系统、子进程、剪贴板、原生窗口——只能通过 `Coconut` 间接做。

## 7. 调试与日志

- **日志文件**：`%APPDATA%\com.coconut.launcher\logs\plugins.log`，
  每行前缀是 UTC 时间（`2026-10-29 11:40:01Z`），超过 2 MB 滚成 `plugins.log.1`。
  **每次打开工具箱都会换新的一份**：旧文件改名成 `plugins-<时间>.log` 备份，只保留最近 10 份。
- **只读 Console**：在工具箱页面按 `Ctrl+T`（或 ``Ctrl+` ``）打开，居中面板，能看
  「插件日志 / 启动器日志」两个来源、每秒自动刷新；没有输入框，只读，`Esc` 或 `✕` 关闭。
- 启动页「插件」弹窗：标题栏右上角 ✕ / `Esc` 关闭；支持按名称 / id / 描述 / 权限搜索、
  按「全部 / 已启用 / 已停用 / 异常」筛选、一键全部启用 / 全部停用；每行右侧是启用开关
  + 重载（**不能在这里删插件**，删文件请自己进插件目录操作；内置插件要「伪删除」就把
  `plugins/state.json` 里那项改成 `"enabled": false, "removed": true`，重启后列表里消失）；
  出错的行会显示「错误 N 次：…」，标题栏计数也会转红。
- 启动器自身 panic：`%APPDATA%\com.coconut.launcher\panic.log`。

## 8. 类型检查（可选但建议）

类型声明**不再随启动器释放**，在仓库的 `plugin-api/` 里（`coconut-plugin.d.ts` +
`tools.d.ts` + `deno.json`）。要用的话把整个目录拷到插件旁边，然后（Deno 2）：

```text
deno check --config plugin-api/deno.json my-plugin.js
```

- 插件文件开头加 `// @ts-check` 参与检查；
- 或者加一行三斜线引用：`/// <reference path="./plugin-api/coconut-plugin.d.ts" />`；
- `noImplicitAny` 故意关掉，参数个数 / 类型写错仍然会报；
- `tools.d.ts` 是生成的，别手改；工具箱更新后用仓库里
  `node plugin-api/generate-tools.mjs [--dir <toolbox-static>]` 重新生成，
  再重新打包启动器。

## 9. 常见坑

| 症状 | 原因 / 解决 |
| --- | --- |
| 插件「加载失败」 | 语法错误，或文件带 BOM 且第一行注释被顶掉；先过 `deno check` |
| 钩子完全没被调用 | 写成了 `const onHello = ...`，必须 `async function onHello() {}` |
| `Coconut.runJs` 返回 `"null"` | 代码里没有 `return`；也别忘了它返回的是字符串 |
| 报「工具箱页面还没打开」 | 在 `onStart` 里就 `runJs` 了；挪到 `onHello` 或按示例 5 重试 |
| 连续 3 次报错被停用 | 钩子抛异常；卡片上看最后一次错误，修完点「启用」 |
| 钩子「超过 10s 未返回」 | 死循环或等一个不会完成的 Promise；别忙等，用定时器 |
| storage 值读出来不对 | 值必须 JSON 可序列化 |
| 改了文件没生效 | 最多 3 秒自动扫描；也可以点「重新扫描」/「重载」 |
| `Coconut.log(obj)` 打出 `[object Object]` | `Coconut.log` 用 `String()`；用 `console.log` 或 `JSON.stringify` |
| 浮层里的按钮点了没反应 | 浮层容器默认 `pointer-events: none`，给可交互元素加 `pointer-events: auto` |

## 10. 安全与礼仪

- 插件能在用户机器上跑、能读工具箱的 AI 配置、能在页面里执行任意 JS：不要偷偷改配置、
  不要上传内容、不要常驻高频轮询。
- 能走启动器本地接口（`http://127.0.0.1:47821/v1/models` 等）就别外连，申请 `network`
  前想清楚。
- `debug` 只在开发时开。
- `onHello` 每次页面就绪都会跑，别在里面做重活，也别假设只跑一次。

## 11. 和旧文档的差异（改版清单）

| 旧 | 新 |
| --- | --- |
| `ctx.runJs` / `Oc*` 类型前缀 | 统一 `Coconut` / `Coconut*` |
| `@apiVersion` 头部 | 已废除，解析器不再读 |
| `notify` 权限 + `Coconut.notify(text)` | 已删除，画浮层或写日志代替 |
| 日志在「插件」弹窗的日志面板 | 弹窗里没有日志入口；工具箱里 `Ctrl+T` 看只读 Console，或直接看文件 |
| 内置 WS 终端（Ctrl+T） | 命令行终端已移除；`Ctrl+T` 现在是只读日志 Console |
| 文档释放到 `plugins/api/` | 改为只留在仓库 `plugin-api/`，不打包进 exe |
| `Coconut.plugin` 含 `apiVersion` | 现在是 `{ id, name, version, mount, permissions }` |
| `ui.panel / ui.close` | 换成 `ui.overlay({ html, css })` + `visible / show / hide / remove` |
| 自己包 sleep | `await Coconut.sleep(ms)` |
| 内置插件不可删 | 启动器里一律不提供删除（用户插件也没有删除按钮） |
