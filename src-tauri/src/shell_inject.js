/*
 * 由启动器通过 WS 调用 run_js 注入到工具箱页面。
 *
 * 目标：
 *   - sidebar 沉浸（窗口无系统标题栏，页面直接顶到边缘）；
 *   - welcome 页顶部留出与「活动工程顶部工具栏」等高的 padding，该区域可拖动窗口；
 *   - 移除活动工程顶部工具栏里的「WS 远程接管 / 使用帮助」按钮；
 *   - 拖动区右侧：缩放 -/+、搜索、下载、最小化、缩放、关闭，图标与下方背景反色。
 *
 * 注意：run_js 的 windowRun 用 new Function 在页面上下文执行，能拿到 DOM。
 * 这里必须幂等：SPA 重渲染 / 二次注入都不能重复挂载。
 */
(() => {
  if (window.__COCONUT_SHELL__) {
    try {
      window.__COCONUT_SHELL__.sync();
    } catch (error) {
      /* 幂等，忽略 */
    }
    return "attached";
  }

  const DEFAULT_H = 44;

  // —— WS 桥：直接连启动器的窗口服务 ——
  const params = new URLSearchParams((location.hash || "").replace(/^#/, ""));
  const target = (params.get("websocket") || "").trim() || "127.0.0.1:8765";
  let socket = null;
  let queue = [];
  let zoom = 1;
  const ZOOM_MIN = 0.9;
  const ZOOM_MAX = 1.2;
  const ZOOM_STEP = 0.05;

  const flush = () => {
    if (!socket || socket.readyState !== 1) return;
    const pending = queue;
    queue = [];
    for (const text of pending) {
      try {
        socket.send(text);
      } catch (error) {
        /* 断开后重连会重发不了，忽略 */
      }
    }
  };

  const sendMessage = (payload) => {
    const text = JSON.stringify(payload);
    queue.push(text);
    if (queue.length > 24) queue.shift();
    flush();
  };

  const send = (action, extra) => {
    sendMessage(Object.assign({ type: "window", action }, extra || {}));
  };

  const connect = () => {
    if (socket && (socket.readyState === 0 || socket.readyState === 1)) return;
    try {
      socket = new WebSocket("ws://" + target);
    } catch (error) {
      socket = null;
      setTimeout(connect, 1500);
      return;
    }
    socket.onopen = () => {
      flush();
      // 页面可能被重新注入（缩放值在启动器那边），先同步一次当前比例。
      send("state");
    };
    socket.onclose = () => {
      socket = null;
      setTimeout(connect, 1500);
    };
    socket.onerror = () => {};
    socket.onmessage = (event) => {
      let message = null;
      try {
        message = JSON.parse(event.data);
      } catch (error) {
        return;
      }
      if (message && message.type === "window-state") {
        zoom = Number(message.zoom);
        if (!Number.isFinite(zoom) || zoom <= 0) zoom = 1;
      }
      if (message && message.type === "log-lines") {
        logConsole.render(message);
      }
    };
  };

  // —— 缩放：Ctrl + - / =，单击 5%，长按线性加速 ——
  const applyZoom = (direction) => {
    const next = Math.min(
      ZOOM_MAX,
      Math.max(ZOOM_MIN, Math.round((zoom + direction * ZOOM_STEP) * 100) / 100),
    );
    if (next === zoom) return;
    zoom = next;
    send("zoom-set", { zoom: next });
  };

  const zoomDirection = (event) => {
    if (!event.ctrlKey || event.altKey || event.metaKey) return 0;
    if (event.code === "Minus" || event.code === "NumpadSubtract") return -1;
    if (event.code === "Equal" || event.code === "NumpadAdd") return 1;
    if (event.key === "-" || event.key === "_") return -1;
    if (event.key === "=" || event.key === "+") return 1;
    return 0;
  };

  let zoomHoldTimer = 0;
  let zoomRampTimer = 0;

  const stopZoomHold = () => {
    clearTimeout(zoomHoldTimer);
    clearInterval(zoomRampTimer);
    zoomHoldTimer = 0;
    zoomRampTimer = 0;
  };

  document.addEventListener(
    "keydown",
    (event) => {
      const direction = zoomDirection(event);
      if (!direction) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return; // 系统重复不管，长按靠自己的 ramp
      applyZoom(direction);
      stopZoomHold();
      zoomHoldTimer = setTimeout(() => {
        zoomRampTimer = setInterval(() => applyZoom(direction), 110);
      }, 420);
    },
    true,
  );

  document.addEventListener(
    "keyup",
    (event) => {
      if (zoomDirection(event)) stopZoomHold();
    },
    true,
  );

  window.addEventListener("blur", stopZoomHold);

  // —— 只读 Console：Ctrl+T 打开，显示启动器的日志文件 ——
  // 只读：没有输入框，只有「插件日志 / 启动器日志」两个来源和自动刷新。
  const logConsole = (() => {
    const MONO =
      '"Cascadia Mono", "JetBrains Mono", "SFMono-Regular", Consolas, "Liberation Mono", monospace';
    let backdrop = null;
    let root = null;
    let body = null;
    let state = null;
    let source = "plugins";
    let visible = false;
    let timer = 0;
    let lastText = "";

    const ensureStyle = () => {
      if (document.getElementById("coconut-log-style")) return;
      const style = document.createElement("style");
      style.id = "coconut-log-style";
      style.textContent = [
        "#coconut-log-backdrop,#coconut-log,#coconut-log *{box-sizing:border-box}",
        "#coconut-log .coconut-log-body::-webkit-scrollbar{width:8px;height:0}",
        "#coconut-log .coconut-log-body::-webkit-scrollbar-thumb{background:#242b24;border-radius:999px}",
        "#coconut-log .coconut-log-body::-webkit-scrollbar-track{background:transparent}",
        "#coconut-log .coconut-log-tab{border:0;background:transparent;color:#6b7468;font:inherit;font-size:11px;",
        "letter-spacing:.06em;cursor:pointer;padding:3px 8px;border-radius:3px}",
        "#coconut-log .coconut-log-tab[data-on='true']{background:#242b24;color:#c9d4c9}",
        "#coconut-log .coconut-log-btn{border:0;background:transparent;color:#6b7468;font:inherit;font-size:11px;",
        "letter-spacing:.06em;cursor:pointer;padding:3px 7px;border-radius:3px}",
        "#coconut-log .coconut-log-btn:hover{background:#242b24;color:#c9d4c9}",
      ].join("");
      (document.head || document.documentElement).appendChild(style);
    };

    const paint = () => {
      body.textContent = lastText || "（暂时没有日志）";
      const nearBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 60;
      if (nearBottom) body.scrollTop = body.scrollHeight;
      state.textContent = (source === "service" ? "启动器日志" : "插件日志") + " · 只读 · 自动刷新";
      for (const tab of root.querySelectorAll(".coconut-log-tab")) {
        tab.dataset.on = tab.dataset.source === source ? "true" : "false";
      }
    };

    const build = () => {
      if (root && root.isConnected) return root;
      ensureStyle();

      backdrop = document.createElement("div");
      backdrop.id = "coconut-log-backdrop";
      backdrop.style.cssText =
        "position:fixed;inset:0;z-index:2147483000;display:none;background:rgba(0,0,0,.5)";
      backdrop.addEventListener("click", (event) => {
        if (event.target === backdrop) toggle(false);
      });

      root = document.createElement("div");
      root.id = "coconut-log";
      root.style.cssText = [
        "position:fixed",
        "left:50%",
        "top:50%",
        "transform:translate(-50%,-50%)",
        "width:min(1020px,92vw)",
        "height:min(660px,82dvh)",
        "z-index:2147483001",
        "display:none",
        "flex-direction:column",
        "overflow:hidden",
        "background:#070907",
        "border:1px solid #242b24",
        "border-radius:5px",
        "box-shadow:0 40px 120px rgba(0,0,0,.7)",
        "color:#b9c2b9",
        "font:12.5px/1.6 " + MONO,
        "-webkit-font-smoothing:antialiased",
      ].join(";");

      const head = document.createElement("div");
      head.style.cssText =
        "display:flex;align-items:center;gap:10px;padding:8px 12px;background:#161a16;" +
        "border-bottom:1px solid #242b24;flex:0 0 auto;user-select:none";
      const title = document.createElement("span");
      title.textContent = "coconut-shell — 日志";
      title.style.cssText =
        "font-size:11.5px;letter-spacing:.06em;color:#788178;white-space:nowrap";
      state = document.createElement("span");
      state.style.cssText =
        "font-size:10.5px;letter-spacing:.05em;color:#4d554d;white-space:nowrap;margin-left:auto";

      const makeTab = (value, label) => {
        const tab = document.createElement("button");
        tab.type = "button";
        tab.className = "coconut-log-tab";
        tab.dataset.source = value;
        tab.textContent = label;
        tab.addEventListener("click", () => {
          source = value;
          sendMessage({ type: "log", action: "tail", source, limit: 400 });
        });
        return tab;
      };

      const close = document.createElement("button");
      close.type = "button";
      close.className = "coconut-log-btn";
      close.textContent = "✕";
      close.style.fontSize = "12px";
      close.title = "关闭（Ctrl+T）";
      close.addEventListener("click", () => toggle(false));

      head.append(title, makeTab("plugins", "插件日志"), makeTab("service", "启动器日志"), state, close);

      body = document.createElement("pre");
      body.className = "coconut-log-body";
      body.style.cssText =
        "flex:1 1 auto;overflow:auto;margin:0;padding:12px 14px;white-space:pre-wrap;" +
        "word-break:break-all;color:#8b948b";

      const hint = document.createElement("div");
      hint.textContent = "只读 · 每秒刷新 · Ctrl+T 关闭 · Esc 关闭";
      hint.style.cssText =
        "flex:0 0 auto;padding:5px 14px 7px;background:#0a0c0a;border-top:1px solid #1b211b;" +
        "color:#3f463f;font-size:10.5px;letter-spacing:.04em";

      root.append(head, body, hint);
      document.body.append(backdrop, root);
      return root;
    };

    const request = () => {
      sendMessage({ type: "log", action: "tail", source, limit: 400 });
    };

    const toggle = (next) => {
      visible = next === undefined ? !visible : Boolean(next);
      build();
      root.style.display = visible ? "flex" : "none";
      backdrop.style.display = visible ? "block" : "none";
      clearInterval(timer);
      if (visible) {
        request();
        paint();
        timer = setInterval(request, 1000);
      }
    };

    const render = (message) => {
      if (!visible) return;
      const lines = Array.isArray(message.lines) ? message.lines : [];
      lastText = lines.join("\n");
      paint();
    };

    return { toggle, render, visible: () => visible };
  })();

  // Ctrl+T / Ctrl+` 开关只读 Console（捕获阶段拦下，不透传给工具箱）
  const isConsoleShortcut = (event) => {
    if (!event.ctrlKey || event.altKey || event.metaKey) return false;
    if (event.code === "KeyT" || event.key === "t" || event.key === "T") return true;
    if (event.code === "Backquote" || event.key === "`" || event.key === "~") return true;
    return false;
  };

  document.addEventListener(
    "keydown",
    (event) => {
      if (event.key === "Escape" && logConsole.visible()) {
        event.preventDefault();
        event.stopPropagation();
        logConsole.toggle(false);
        return;
      }
      if (!isConsoleShortcut(event)) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      logConsole.toggle();
    },
    true,
  );

  // —— 样式 ——
  const style = document.createElement("style");
  style.id = "coconut-shell-style";
  style.textContent = [
    ":root{--coconut-titlebar-h:" + DEFAULT_H + "px;}",
    // 工具栏上这两个按钮由启动器接管（WS 由启动器常驻，帮助改由沉浸式外壳承担）
    'button[title^="WS 远程接管"],button[title="使用帮助"]{display:none !important;}',
    // 没有活动工程时（welcome 页）给内容顶部留出拖动条的高度
    'body:not(:has([data-sf="toolbar"])) [data-sf="content"] > div:first-child{padding-top:calc(var(--coconut-titlebar-h) + 1.75rem) !important;}',
    "#coconut-window-controls{position:fixed;top:0;right:6px;height:var(--coconut-titlebar-h);display:flex;align-items:center;gap:1px;",
    "z-index:2147482000;user-select:none;-webkit-user-select:none;mix-blend-mode:difference;color:#fff;}",
    "#coconut-window-controls button{display:grid;place-items:center;width:28px;height:24px;padding:0;margin:0;border:0;",
    "border-radius:5px;background:transparent;color:inherit;cursor:pointer;font:inherit;}",
    "#coconut-window-controls button:hover{background:rgba(255,255,255,0.22);}",
    "#coconut-window-controls button:active{background:rgba(255,255,255,0.34);}",
    "#coconut-window-controls svg{width:13px;height:13px;display:block;}",
    "html.coconut-dragging,html.coconut-dragging *{cursor:grabbing !important;user-select:none !important;}",
    // AI 助手悬浮窗：高度让出沉浸式标题栏，避免和窗口控制区重叠
    ".ai-assistant{max-height:calc(100dvh - var(--coconut-titlebar-h) - 16px) !important;}",
    // 提示位置被强制到左下角时生效（右上角会压住窗口控制区）
    "html.coconut-toast-bottom-left div.pointer-events-none.fixed.flex.flex-col.gap-2{",
    "top:auto !important;bottom:1rem !important;left:1rem !important;right:auto !important;",
    "align-items:flex-start !important;}",
  ].join("");

  // —— 图标 ——
  const icon = (paths) =>
    '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" ' +
    'stroke-linecap="round" stroke-linejoin="round">' +
    paths +
    "</svg>";

  const ICONS = {
    minimize: icon('<path d="M3 8.5h10"/>'),
    maximize: icon('<rect x="3.4" y="3.4" width="9.2" height="9.2" rx="1.3"/>'),
    close: icon('<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>'),
  };

  const cluster = document.createElement("div");
  cluster.id = "coconut-window-controls";
  cluster.innerHTML = [
    '<button type="button" data-act="minimize" title="最小化">' + ICONS.minimize + "</button>",
    '<button type="button" data-act="maximize" title="缩放窗口">' + ICONS.maximize + "</button>",
    '<button type="button" data-act="close" title="关闭">' + ICONS.close + "</button>",
  ].join("");

  cluster.addEventListener("click", (event) => {
    const button = event.target.closest("button[data-act]");
    if (!button) return;
    event.preventDefault();
    event.stopPropagation();
    switch (button.dataset.act) {
      case "minimize":
        send("minimize");
        break;
      case "maximize":
        send("maximize-toggle");
        break;
      case "close":
        send("close");
        break;
      default:
        break;
    }
  });

  // 阻止控制簇把事件冒泡给工具箱（避免触发行点击 / 快捷键）
  for (const type of ["pointerdown", "dblclick", "mousedown"]) {
    cluster.addEventListener(type, (event) => event.stopPropagation());
  }

  // —— 拖动窗口 ——
  const INTERACTIVE =
    'button,a,input,select,textarea,label,[data-press],[role="button"],[role="slider"],[contenteditable="true"],#coconut-window-controls';

  const isInteractive = (node) => {
    if (!node || !node.closest) return false;
    return Boolean(node.closest(INTERACTIVE));
  };

  /// sidebar 右侧边界：sidebar 顶部是「回到工作台」的入口，绝不能当作拖动区。
  const sidebarRight = () => {
    const sidebar = document.querySelector('aside[data-sf="sidebar"]');
    if (!sidebar) return 0;
    const rect = sidebar.getBoundingClientRect();
    return rect.width > 0 ? rect.right : 0;
  };

  const inDragBand = (event) => {
    if (event.clientY > titlebarHeight()) return false;
    if (event.clientX <= sidebarRight()) return false;
    return true;
  };

  const onPointerDown = (event) => {
    if (event.button !== 0) return;
    if (!inDragBand(event)) return;
    if (isInteractive(event.target)) return;
    document.documentElement.classList.add("coconut-dragging");
    setTimeout(() => document.documentElement.classList.remove("coconut-dragging"), 500);
    send("drag");
    event.preventDefault();
  };

  const onDoubleClick = (event) => {
    if (!inDragBand(event)) return;
    if (isInteractive(event.target)) return;
    send("maximize-toggle");
  };

  // —— 数据上游显示成 local ——
  // 工具箱写死了 `Nanoka (nanoka.cc)` 这一套标签，本地数据由启动器兜底，
  // 直接把它看到的文案改写成 local。
  const PROVIDER_PATTERN = /nanoka/i;
  const rewriteProviders = (root) => {
    if (!root || !document.createTreeWalker) return;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    let node = walker.nextNode();
    while (node) {
      const text = node.nodeValue;
      if (text && PROVIDER_PATTERN.test(text)) {
        node.nodeValue = text
          .replace(/Nanoka\s*\(nanoka\.cc\)/gi, "local")
          .replace(/nanoka\.cc/gi, "local")
          .replace(/nanoka/gi, "local");
      }
      node = walker.nextNode();
    }
  };

  // —— 强制开启「WS 远程接管」——
  // 启动器靠它下发 run_js / 工具调用，关掉就等于把启动器的手砍了。
  // 做法：把持久化开关写成 1（刷新后自动重连），并盯着它，
  // 一旦被改成 0 就改回来 + 重新挂 #websocket= hash（工具箱监听 hashchange 会重连）。
  const WS_FLAG = "wuwa-afyg:ws-prefs:enabled";
  const WS_URL = "wuwa-afyg:ws-prefs:url";
  const forceWsEnabled = (reconnect) => {
    try {
      if (localStorage.getItem(WS_FLAG) !== "1") localStorage.setItem(WS_FLAG, "1");
      if (!(localStorage.getItem(WS_URL) || "").trim()) localStorage.setItem(WS_URL, target);
    } catch (error) {
      /* 隐私模式等：忽略 */
    }
    if (!reconnect) return;
    try {
      if (location.hash !== "#websocket=" + target) {
        location.hash = "websocket=" + target;
      }
    } catch (error) {
      /* 忽略 */
    }
  };

  const keepWsEnabled = () => {
    let flag = "1";
    try {
      flag = localStorage.getItem(WS_FLAG) || "1";
    } catch (error) {
      return;
    }
    if (flag !== "1") {
      forceWsEnabled(true);
    }
  };

  // —— 隐藏设置里的「WS 远程接管」（开关 + 地址 + 状态整块）——
  const WS_PANEL_TITLE = "WS 远程接管";
  const normalizeText = (text) => (text || "").replace(/\s+/g, " ").trim();
  const hideWsPanel = () => {
    // 设置 → 连接配置 里那一整块（标题 + 开关 + 地址 + 状态）是
    // `<div class="border-t pt-5">`；用「以标题开头 + 文本不长」两个条件兜住，
    // 既不怕 Svelte 改类名，也不会误伤整个设置弹窗。
    for (const node of document.querySelectorAll('div[class*="border-t"]')) {
      if (node.dataset.coconutWsHidden === "1") continue;
      const text = normalizeText(node.textContent);
      if (!text.startsWith(WS_PANEL_TITLE)) continue;
      if (text.length > 800) continue;
      node.dataset.coconutWsHidden = "1";
      node.style.display = "none";
    }
  };

  // —— 屏蔽工具箱自带的「缓存清理」入口 ——
  // 它其实是设置弹窗左侧导航里的一个按钮（数据 → 连接配置 / 缓存清理 / 归档管理）。
  // 数据由启动器本地提供，清缓存只会让工具箱重新拉一遍，还容易让人以为删了本地数据。
  const CACHE_TITLE = "缓存清理";
  const hideCachePanel = () => {
    for (const button of document.querySelectorAll("button")) {
      if (button.dataset.coconutHidden) continue;
      if ((button.textContent || "").trim() !== CACHE_TITLE) continue;
      button.dataset.coconutHidden = "1";
      button.style.display = "none";
    }
    // 兜底：万一从别的路径进到缓存面板，把清理按钮也藏掉
    for (const button of document.querySelectorAll("button")) {
      const label = (button.textContent || "").trim();
      if (label !== "全部清理" && label !== "清理本类") continue;
      if (button.dataset.coconutHidden) continue;
      button.dataset.coconutHidden = "1";
      button.style.display = "none";
    }
  };

  // —— AI 助手悬浮窗：别顶到沉浸式标题栏下面 ——
  // 工具箱全尺寸时高度是 window.innerHeight - 24，位置还可能被拖到 y≈12，
  // 正好压住我们的标题栏。CSS 限制最大高度，并把 top 顶到标题栏下方。
  const clampAssistant = () => {
    const panel = document.querySelector(".ai-assistant");
    if (!panel) return;
    const minTop = titlebarHeight() + 8;
    const top = Number.parseFloat(panel.style.top || "");
    if (Number.isFinite(top) && top < minTop) {
      panel.style.top = `${Math.round(minTop)}px`;
    }
  };

  // —— 预热图标缓存 ——
  // 启动器已经把 batch-icons 本地化成 /icons/*.webp，这里把同一批 URL 塞进工具箱
  // 自己的 CacheStorage（data-cdn），工具箱就不会再为这些图发请求。
  const WARM_FLAG = "coconut-shell:icons-warmed";
  const warmIconCache = async () => {
    if (typeof caches === "undefined") return;
    try {
      if (sessionStorage.getItem(WARM_FLAG)) return;
      sessionStorage.setItem(WARM_FLAG, "1");
    } catch (error) {
      return;
    }

    const urls = new Set();
    for (const entity of ["character", "weapon", "echo", "echo-set"]) {
      try {
        const response = await fetch(`/api/v1/batch-icons/${entity}`, { cache: "no-store" });
        if (!response.ok) continue;
        const payload = await response.json();
        if (payload && typeof payload === "object") {
          for (const value of Object.values(payload)) {
            if (typeof value === "string" && value.startsWith("http://127.0.0.1")) {
              urls.add(value);
            }
          }
        }
      } catch (error) {
        /* 忽略，单个表失败不影响其它 */
      }
    }
    if (urls.size === 0) return;

    const cache = await caches.open("data-cdn");
    let added = 0;
    for (const url of urls) {
      try {
        const hit = await cache.match(url);
        if (hit) continue;
        const response = await fetch(url);
        if (!response.ok) continue;
        await cache.put(url, response);
        added += 1;
      } catch (error) {
        /* 单张失败就跳过 */
      }
    }
    if (added > 0) console.info(`[coconut-shell] 已预热 ${added} 张图标到缓存`);
  };

  // —— Toast 位置：右上角会压住沉浸式标题栏，强制改成左下角 ——
  const TOAST_KEY = "wuwa-afyg:interaction-prefs:toast-position";
  const TOAST_FORCED = "bottom-left";
  let toastForced = false;

  const readToast = () => {
    try {
      return localStorage.getItem(TOAST_KEY) || "";
    } catch (error) {
      return "";
    }
  };

  const applyToastOverride = () => {
    const value = readToast();
    // 用户之后自己改了设置（值已经不是我们强制的那个）→ 交还给工具箱。
    if (toastForced && value !== TOAST_FORCED) {
      toastForced = false;
    }
    // 默认值 / 右上角 → 强制到左下角，并写回设置让下次启动就位。
    if (!toastForced && (value === "" || value === "top-right")) {
      toastForced = true;
      try {
        localStorage.setItem(TOAST_KEY, TOAST_FORCED);
      } catch (error) {
        /* 存不进去也不影响当前会话的强制显示 */
      }
    }
    document.documentElement.classList.toggle("coconut-toast-bottom-left", toastForced);
  };

  // —— 工坊 iframe：改走本机同站反代 ——
  // 工坊域和工具箱是跨站，会话 Cookie 在 iframe 里会被 SameSite 拦掉；
  // 换成 http://127.0.0.1:47822（同站、不同端口）后 Cookie 就正常了。
  // 上游地址由 iframe 自己的 src 决定，所以任何用户填的工坊实例都会被反代。
  const WORKSHOP_PROXY = "127.0.0.1:47822";
  const rewriteWorkshop = () => {
    for (const frame of document.querySelectorAll('iframe[title="椰果工坊"]')) {
      const source = frame.getAttribute("src") || "";
      if (!/^https?:\/\//i.test(source)) continue;
      let url;
      try {
        url = new URL(source);
      } catch (error) {
        continue;
      }
      if (url.host === WORKSHOP_PROXY) continue;
      if (url.hostname === "127.0.0.1" || url.hostname === "localhost") continue;
      const query = new URLSearchParams(url.search);
      query.set("__coconut_target", `${url.protocol}//${url.host}`);
      const next = `http://${WORKSHOP_PROXY}${url.pathname}?${query.toString()}${url.hash}`;
      frame.setAttribute("src", next);
      console.info("[coconut-shell] 工坊 iframe 已改走同站反代：", url.origin);
    }
  };

  // —— 高度同步 ——
  let barHeight = DEFAULT_H;
  let lastMeasure = 0;
  const titlebarHeight = () => barHeight;

  const measure = () => {
    const now = Date.now();
    if (now - lastMeasure < 400) return barHeight;
    lastMeasure = now;
    const bar = document.querySelector('[data-sf="toolbar"]:not([role="toolbar"])');
    const raw = bar ? Math.round(bar.getBoundingClientRect().height) : 0;
    const next = raw >= 28 && raw <= 90 ? raw : DEFAULT_H;
    if (next !== barHeight) {
      barHeight = next;
      document.documentElement.style.setProperty("--coconut-titlebar-h", next + "px");
    }
    return barHeight;
  };

  // —— 内置 WS 终端 / 插件浮层 / 提示条 ——
  // —— 插件浮层宿主 ——
  // 插件只负责「给一段 html + 一段 css」，位置/层级由引擎管：
  //   __coconutOverlayRegister(html, css) → 浮层 id（递增）
  //   __coconutOverlayVisible(id, visible) → 显示 / 隐藏
  //   __coconutOverlayRemove(id)           → 删掉
  // 浮层铺满视口、默认不拦截鼠标，插件自己给内容设 pointer-events:auto。
  const OVERLAY_BASE_Z = 2147481000;
  let overlaySeq = 0;
  const overlays = new Map();

  window.__coconutOverlayRegister = (plugin, payload) => {
    const options = payload && typeof payload === "object" ? payload : {};
    const id = ++overlaySeq;
    const root = document.createElement("div");
    root.dataset.coconutOverlay = String(id);
    root.dataset.coconutPlugin = String(plugin || "");
    // 层级由引擎统一编排：谁后注册谁在上面
    root.style.cssText =
      "position:fixed;inset:0;z-index:" + (OVERLAY_BASE_Z + id) +
      ";pointer-events:none;display:block";
    if (options.css) {
      const style = document.createElement("style");
      style.textContent = String(options.css);
      root.appendChild(style);
    }
    if (options.html) {
      const body = document.createElement("div");
      body.style.cssText = "position:absolute;inset:0;pointer-events:none";
      body.innerHTML = String(options.html);
      root.appendChild(body);
    }
    document.body.appendChild(root);
    overlays.set(id, { root: root, plugin: String(plugin || ""), visible: true });
    return id;
  };

  window.__coconutOverlayVisible = (id, visible) => {
    const entry = overlays.get(Number(id));
    if (!entry) return false;
    entry.visible = Boolean(visible);
    entry.root.style.display = entry.visible ? "block" : "none";
    return true;
  };

  window.__coconutOverlayRemove = (id) => {
    const key = Number(id);
    const entry = overlays.get(key);
    if (!entry) return false;
    entry.root.remove();
    overlays.delete(key);
    return true;
  };

  window.__coconutInjectCss = (plugin, id, css) => {
    const key = "css::" + String(plugin) + "::" + String(id);
    let node = document.getElementById(key);
    if (!node) {
      node = document.createElement("style");
      node.id = key;
      (document.head || document.documentElement).appendChild(node);
    }
    node.textContent = String(css || "");
    return true;
  };

  // 插件停用 / 重载时清掉它注册过的浮层，避免残影
  window.__coconutOverlayClearPlugin = (plugin) => {
    const name = String(plugin || "");
    for (const [id, entry] of overlays.entries()) {
      if (entry.plugin === name) {
        entry.root.remove();
        overlays.delete(id);
      }
    }
    return true;
  };

  const sendRaw = (payload) => {
    const text = JSON.stringify(payload);
    queue.push(text);
    if (queue.length > 64) queue.shift();
    flush();
  };

  // —— 挂载 / 自愈 ——
  let scheduled = false;
  let lastHeavy = 0;
  const sync = () => {
    if (!style.isConnected) (document.head || document.documentElement).appendChild(style);
    if (!cluster.isConnected) document.body.appendChild(cluster);
    measure();
    clampAssistant();
    // 全量文本改写 / 面板扫描比较重，限制到 4 次/秒
    const now = Date.now();
    if (now - lastHeavy >= 250) {
      lastHeavy = now;
      applyToastOverride();
      rewriteProviders(document.body);
      hideCachePanel();
      hideWsPanel();
      keepWsEnabled();
      rewriteWorkshop();
    }
  };

  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      try {
        sync();
      } catch (error) {
        /* 忽略 */
      }
    });
  };

  document.addEventListener("pointerdown", onPointerDown, true);
  document.addEventListener("dblclick", onDoubleClick, true);

  const observer = new MutationObserver(schedule);
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    characterData: true,
    attributes: true,
    attributeFilter: ["style", "class"],
  });
  setInterval(() => {
    measure();
    applyToastOverride();
    hideCachePanel();
    hideWsPanel();
    keepWsEnabled();
    clampAssistant();
    rewriteWorkshop();
    if (!style.isConnected || !cluster.isConnected) schedule();
  }, 800);

  forceWsEnabled(false);
  sync();
  connect();
  setTimeout(() => {
    warmIconCache().catch(() => {});
  }, 1500);

  window.__COCONUT_SHELL__ = { sync: schedule, send, measure };
  return "installed";
})();
