/**
 * 椰果工具箱 · 插件 API v1
 */

import type { CoconutToolName, CoconutToolParams } from './tools.d.ts';

export type { CoconutToolName, CoconutToolParams };

/* ───────────────────────────── 元数据 / 权限 ───────────────────────────── */

/** 插件能申请的权限；没声明就调用对应 API 会抛错。 */
export type CoconutPermission =
  /** Coconut.runJs：在工具箱页面里执行 JS */
  | 'dom'
  /** Coconut.callTool：调用工具箱的 WS 工具 */
  | 'toolbox'
  /** Coconut.fetch：由启动器代发 HTTP（不限域名） */
  | 'network'
  /** Coconut.storage：插件私有 KV（落盘到 plugins/storage/<id>.json） */
  | 'storage'
  /** Coconut.ui：浮层 / 注入 CSS（同时需要 dom） */
  | 'ui'
  /** 只读订阅原始 WS 收发帧（开发调试用，默认先别申请） */
  | 'debug';

/** 露脸方式。目前只影响启动页插件列表里的展示，不改变 API。 */
export type CoconutMount = 'none' | 'overlay';

/** Coconut.plugin：启动器解析出的元数据。 */
export interface CoconutPluginMeta {
  /** 插件 id = 文件名（不含 .js） */
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly mount: CoconutMount;
  readonly permissions: CoconutPermission[];
}

/* ───────────────────────────── 事件 ───────────────────────────── */

/** onHello 的参数：工具箱页面就绪。 */
export interface CoconutHelloInfo {
  /** 这一条 WS 会话的 id（字符串） */
  session: string;
  /** 工具箱当前注册的工具名列表（含 run_js 等） */
  tools: string[];
}

/** 启动器主动触发「重新同步」（免费模型插件就靠这个）。 */
export interface CoconutResyncEvent {
  type: 'resync';
  /** 可选：模型列表已就绪时带上的 id 数组 */
  models?: string[];
}

/** 原始 WS 帧（需要 debug 权限）。direction = recv 是工具箱发来的，send 是启动器发出去的。 */
export interface CoconutFrameEvent {
  type: 'frame';
  direction: 'recv' | 'send';
  frame: unknown;
  /** 毫秒时间戳 */
  at: number;
}

/**
 * 目前会收到的全部事件。
 *
 * 按 `event.type` 窄化即可；以后新增事件类型时会直接多出新成员，
 * 所以请把不认识的 event 直接忽略掉，不要假设只会有这两种。
 */
export type CoconutEvent = CoconutResyncEvent | CoconutFrameEvent;

/* ───────────────────────────── 能力 ───────────────────────────── */

/** Coconut.fetch 的选项（走启动器进程里的 reqwest，不受页面 CORS 限制）。 */
export interface CoconutFetchOptions {
  /** 默认 GET；大小写不敏感 */
  method?: string;
  headers?: Record<string, string>;
  /** 原始请求体 */
  body?: string;
  /** JSON 请求体（设置了 body 时优先 body） */
  json?: unknown;
}

/** Coconut.fetch 的返回值。body 是原文，json 是自动 JSON.parse 的结果（解析失败为 null）。 */
export interface CoconutFetchResult {
  status: number;
  ok: boolean;
  headers: Record<string, string>;
  body: string;
  json: unknown;
}

/** 注册浮层时的初始内容。 */
export interface CoconutOverlayOptions {
  /** 初始 HTML */
  html?: string;
  /** 初始 CSS（只作用于这个浮层内部的元素） */
  css?: string;
}

/** 插件私有 KV：值是 JSON 可序列化的东西，落盘在 plugins/storage/<id>.json。 */
export interface CoconutStorage {
  /** 读一个键；不存在时返回 fallback（没给 fallback 则返回 null） */
  get<T = unknown>(key: string): T | null;
  get<T = unknown>(key: string, fallback: T): T;
  set(key: string, value: unknown): true;
  delete(key: string): true;
  /** 读出全部键值（浅拷贝） */
  all(): Record<string, unknown>;
}

/** 插件浮层 / 样式注入。 */
export interface CoconutUi {
  /**
   * 注册一个浮层，返回引擎分配的浮层 id（从 1 递增）。
   *
   * 浮层铺满视口，**层级（z-index）由引擎统一编排**：后注册的盖住先注册的。
   * 浮层本身不拦截鼠标，需要交互的元素自己写 `pointer-events: auto`。
   */
  overlay(options?: CoconutOverlayOptions): number;
  /** 显示 / 隐藏指定浮层；返回是否找到了这个 id。 */
  visible(id: number, visible: boolean): boolean;
  /** 等价于 `visible(id, true)` */
  show(id: number): boolean;
  /** 等价于 `visible(id, false)` */
  hide(id: number): boolean;
  /** 删除指定浮层；返回是否找到了这个 id。 */
  remove(id: number): boolean;
  /**
   * 往工具箱页面注入一段页面级 CSS（不是浮层）。
   * 内容放在 <style id="css::<插件id>::<id>"> 里，重复调用会覆盖，传空串就撤掉。
   */
  injectCss(id: string, css: string): true;
}

/** 启动器信息（只读）。 */
export interface CoconutLauncherInfo {
  /** 启动器当前语言：zh-Hans / zh-Hant / ja / ko / en */
  readonly lang: string;
  /** 椰果工具箱包体版本（commit / build） */
  readonly toolboxVersion: string;
  /** 游戏数据版本 */
  readonly gameVersion: string;
}

/** 一个资源根：本地目录 + 启动器本地服务地址。 */
export interface CoconutSource {
  /** 资源在计算机上的绝对路径（例如 C:\\Users\\...\\toolbox-static） */
  readonly dir: string;
  /** 启动器本地服务里对应的地址，可直接给 fetch / img src 用 */
  readonly url: string;
}

/** 插件读取工具箱资源用的两个根。 */
export interface CoconutSrc {
  /** 椰果工具箱包体目录：dir + http://127.0.0.1:47821/ */
  readonly toolbox: CoconutSource;
  /** 启动器本地资源目录（本地化游戏图标等）：dir + http://127.0.0.1:47821/icons/ */
  readonly static: CoconutSource;
}

/** 插件拿到的全局 `Coconut`。 */
export interface CoconutContext {
  readonly plugin: CoconutPluginMeta;

  /**
   * 在工具箱页面里执行 JS 并等它跑完。
   *
   * ⚠️ 传的是**函数体**，不是表达式：工具箱会把它包成
   * `return (async () => { <你的代码> })()`，所以必须自己 `return`。
   * 有 `await` 也可以（工具箱那边是 async 上下文）。
   *
   * 返回值统一转成字符串：返回对象 → JSON 文本（自己 `JSON.parse`）。
   *
   * @example
   *   const count = Number(Coconut.runJs('return document.querySelectorAll(".card").length'));
   *   const config = JSON.parse(Coconut.runJs(`
   *     const db = await new Promise((resolve) => {
   *       const req = indexedDB.open('wuwa-v1', 1);
   *       req.onsuccess = () => resolve(req.result);
   *       req.onerror = () => resolve(null);
   *     });
   *     return JSON.stringify(db ? db.name : null);
   *   `));
   */
  runJs(code: string): string;

  /**
   * 调用工具箱的任意 WS 工具（名字见工具箱的 AI 工具表）。
   * 返回的是该工具的返回值本身（启动器已经把协议字段剥掉了）。
   *
   * 工具名和入参写在下方的 `tools.d.ts` 里（由工具箱包体生成）：名字能补全、
   * 参数会被校验。返回类型是 `unknown`（工具箱没提供返回结构），要字段自己断言。
   * 工具箱刚更新、`tools.d.ts` 还没重新生成时，可以用
   * `Coconut.callTool('新工具名' as CoconutToolName, {...})` 绕过名字检查。
   *
   * 需要 toolbox 权限。
   */
  callTool<K extends CoconutToolName>(tool: K, args?: CoconutToolParams[K]): unknown;

  /** 由启动器代发 HTTP 请求。需要 network 权限。 */
  fetch(url: string, options?: CoconutFetchOptions): CoconutFetchResult;

  /** 插件私有 KV。需要 storage 权限。 */
  readonly storage: CoconutStorage;

  /** 浮层 / CSS。需要 ui 权限（overlay / injectCss 还需要 dom）。 */
  readonly ui: CoconutUi;

  /** 启动器信息：语言 / 工具箱版本 / 游戏数据版本（只读，不需要权限）。 */
  readonly launcher: CoconutLauncherInfo;

  /** 工具箱包体 + 启动器本地资源（只读，不需要权限）。 */
  readonly src: CoconutSrc;

  /** 写日志：只落盘到 %APPDATA%\com.coconut.launcher\logs\plugins.log。 */
  log(...args: unknown[]): void;

  /** 等一会儿（毫秒）：`await Coconut.sleep(500)`。需要忙等时优先用它。 */
  sleep(ms: number): Promise<void>;

  /**
   * 定时器都在插件线程的事件循环里跑，钩子会等它们跑完；
   * 回调调用时**不带参数**（只是为了能塞 Promise 的 resolve，才允许声明入参）。
   */
  setTimeout(handler: (...args: unknown[]) => void, ms?: number): number;
  setInterval(handler: (...args: unknown[]) => void, ms?: number): number;
  clearTimeout(id: number): void;
  clearInterval(id: number): void;
}

/* ───────────────────────────── 全局声明 ───────────────────────────── */

declare global {
  /** 插件上下文（见上）。 */
  const Coconut: CoconutContext;

  /**
   * 插件钩子。**按需定义**即可，没定义的钩子等于空实现。
   * 定义成普通 function / async function 都行。
   */
  var onStart: (() => void | Promise<void>) | undefined;
  var onHello: ((info: CoconutHelloInfo) => void | Promise<void>) | undefined;
  var onEvent: ((event: CoconutEvent) => void | Promise<void>) | undefined;
  var onStop: (() => void | Promise<void>) | undefined;

  /* 这几个宿主 API 都**没有**：fetch / URL / URLSearchParams / TextEncoder /
     TextDecoder / atob / btoa / structuredClone / crypto / Buffer / process /
     require / import。要发请求请用 Coconut.fetch，要编码请自己写。 */

  /* console.* / setTimeout / setInterval / clearTimeout / clearInterval /
     queueMicrotask 沿用 `lib.dom` 的声明（类型一样），运行时由启动器提供：
     - console.* 转发到插件日志（warn / error 记成同名级别，其余记 info）；
     - 定时器是插件线程自己的事件循环，回调**不带参数**；
     - 其余宿主 API 一律没有，见上面那段注释。 */
}

export {};
