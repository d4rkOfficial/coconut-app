// @name        调试终端
// @version     2.0.0
// @description 可读写调试终端：插件层 / DOM 层 JS、console 输出、括号感知回车
// @permissions dom, network
// @mount       none
//
// 这是内置插件源码。启动器只在插件目录里没有 debug-terminal.js 时导出它；
// 导出之后以插件目录里的文件为准（可以直接改 / 删 / 覆盖）。
//
// 实现上不新增公开 API：
//   - DOM 层用 Coconut.runJs；
//   - 插件层用插件线程自己的 eval；
//   - console.* 由启动器内部 /debug/console 环形缓冲提供，终端用 Coconut.fetch 轮询。

const CONSOLE_ENDPOINT = 'http://127.0.0.1:47821/debug/console';
let pollTimer = null;
let cursor = 0;
let cleared = -1;
let ready = false;
let ticking = false;

const logError = (error) => {
  try {
    Coconut.log('调试终端：' + String((error && error.stack) || error));
  } catch (ignored) {}
};

const pageCall = (fn, ...args) => {
  const list = args.length ? args.map((arg) => JSON.stringify(arg)).join(', ') : '';
  return `return (${String(fn).trim()})(${list});`;
};

const formatValue = (value) => {
  if (typeof value === 'string') return value;
  if (value === undefined) return 'undefined';
  if (typeof value === 'function') return String(value);
  try {
    const text = JSON.stringify(value, null, 2);
    return text === undefined ? String(value) : text;
  } catch (error) {
    return String(value);
  }
};

const feedPage = (text) => {
  if (!window.__coconutTerm) return false;
  window.__coconutTerm.feed(String(text));
  return true;
};

const clearPage = () => {
  if (!window.__coconutTerm) return false;
  window.__coconutTerm.clear();
  return true;
};

const pollPage = () => {
  if (!window.__coconutTerm) return JSON.stringify({ commands: [], visible: false });
  return window.__coconutTerm.poll();
};

const installTerminal = () => {
  if (window.__coconutTerm) {
    if (window.__coconutTerm.host && window.__coconutTerm.host.isConnected) return 'attached';
    try {
      window.__coconutTerm.remove();
    } catch (ignored) {}
  }

  const host = document.createElement('div');
  host.id = 'coconut-term-host';
  host.style.cssText = 'position:fixed;left:0;right:0;bottom:0;z-index:2147483000;display:none;';
  const shadow = host.attachShadow({ mode: 'open' });
  shadow.innerHTML = `
    <style>
      :host { all: initial; }
      .term { box-sizing: border-box; width: 100%; height: min(44dvh, 440px); display: flex;
        flex-direction: column; background: #070907; color: #b9c2b9; border-top: 1px solid #242b24;
        box-shadow: 0 -18px 60px rgba(0,0,0,.55); font: 12.5px/1.55 "Cascadia Mono",
        "JetBrains Mono", "SFMono-Regular", Consolas, "Liberation Mono", monospace;
        -webkit-font-smoothing: antialiased; }
      .head { display: flex; align-items: center; gap: 10px; padding: 7px 12px;
        background: #161a16; border-bottom: 1px solid #242b24; flex: 0 0 auto; user-select: none; }
      .title { color: #788178; font-size: 11.5px; letter-spacing: .06em; white-space: nowrap; }
      .layer { border: 1px solid #334033; background: #0d120d; color: #9fb59f; font-size: 10.5px;
        padding: 1px 7px; border-radius: 3px; cursor: pointer; white-space: nowrap; }
      .hint { color: #4d554d; font-size: 10.5px; white-space: nowrap; margin-left: auto; }
      .btn { border: 0; background: transparent; color: #6b7468; font: inherit; font-size: 11px;
        letter-spacing: .06em; cursor: pointer; padding: 2px 7px; border-radius: 3px; }
      .btn:hover { background: #242b24; color: #c9d4c9; }
      .out { flex: 1 1 auto; overflow: auto; margin: 0; padding: 10px 14px; white-space: pre-wrap;
        word-break: break-word; color: #8b948b; }
      .out::-webkit-scrollbar { width: 8px; }
      .out::-webkit-scrollbar-thumb { background: #242b24; border-radius: 999px; }
      .line { display: block; min-height: 1.55em; }
      .prompt { display: flex; gap: 8px; align-items: flex-start; padding: 6px 14px 10px;
        background: #0a0c0a; border-top: 1px solid #1b211b; flex: 0 0 auto; }
      .sigil { color: #9fb59f; white-space: nowrap; padding-top: 3px; }
      textarea { flex: 1 1 auto; min-height: 22px; max-height: 160px; resize: none; border: 0;
        outline: 0; background: transparent; color: #c9d4c9; font: inherit; line-height: 1.55;
        padding: 2px 0; caret-color: #c9d4c9; }
      .note { color: #3f463f; font-size: 10.5px; padding: 0 14px 7px; background: #0a0c0a; }
    </style>
    <div class="term">
      <div class="head">
        <span class="title">coconut — 调试终端</span>
        <button class="layer" data-layer>插件层</button>
        <span class="hint">Tab 切换层级 · Enter 运行 · 括号未闭合换行 · Ctrl+T 关闭</span>
        <button class="btn" data-clear>清空</button>
        <button class="btn" data-close>✕</button>
      </div>
      <pre class="out"></pre>
      <div class="prompt"><span class="sigil">&gt;</span><textarea rows="1" spellcheck="false"></textarea></div>
      <div class="note">console 输出与运行结果会显示在这里；Coconut.log 仍写入 plugins.log。</div>
    </div>`;

  const root = shadow.querySelector('.term');
  const out = shadow.querySelector('.out');
  const input = shadow.querySelector('textarea');
  const layerButton = shadow.querySelector('[data-layer]');
  const closeButton = shadow.querySelector('[data-close]');
  const clearButton = shadow.querySelector('[data-clear]');
  const state = { visible: false, layer: 'plugin', commands: [], lines: [] };

  const isBalanced = (text) => {
    const stack = [];
    let quote = '';
    let lineComment = false;
    let blockComment = false;
    for (let index = 0; index < text.length; index += 1) {
      const ch = text[index];
      const next = text[index + 1];
      if (lineComment) {
        if (ch === '\n') lineComment = false;
        continue;
      }
      if (blockComment) {
        if (ch === '*' && next === '/') {
          blockComment = false;
          index += 1;
        }
        continue;
      }
      if (quote) {
        if (ch === '\\') index += 1;
        else if (ch === quote) quote = '';
        continue;
      }
      if (ch === '/' && next === '/') {
        lineComment = true;
        index += 1;
        continue;
      }
      if (ch === '/' && next === '*') {
        blockComment = true;
        index += 1;
        continue;
      }
      if (ch === '"' || ch === "'" || ch === '`') {
        quote = ch;
        continue;
      }
      if (ch === '(' || ch === '[' || ch === '{') stack.push(ch);
      else if (ch === ')' || ch === ']' || ch === '}') {
        const open = stack.pop();
        if (!open || (open === '(' && ch !== ')') || (open === '[' && ch !== ']') || (open === '{' && ch !== '}')) return false;
      }
    }
    return !quote && !lineComment && !blockComment && stack.length === 0;
  };

  const append = (text) => {
    const value = String(text === undefined || text === null ? '' : text);
    if (!value) return;
    state.lines.push(value);
    if (state.lines.length > 3000) state.lines.splice(0, state.lines.length - 3000);
    out.textContent = state.lines.join('\n');
    out.scrollTop = out.scrollHeight;
  };

  const setLayer = (layer) => {
    state.layer = layer === 'dom' ? 'dom' : 'plugin';
    layerButton.textContent = state.layer === 'dom' ? 'DOM 层' : '插件层';
  };

  const resizeInput = () => {
    input.style.height = 'auto';
    input.style.height = Math.min(160, input.scrollHeight) + 'px';
    out.scrollTop = out.scrollHeight;
  };

  const submit = () => {
    const code = input.value;
    if (!code.trim()) return;
    state.commands.push({ layer: state.layer, code });
    append('> ' + code);
    input.value = '';
    resizeInput();
  };

  input.addEventListener('keydown', (event) => {
    if (event.key === 'Tab') {
      event.preventDefault();
      event.stopPropagation();
      setLayer(state.layer === 'plugin' ? 'dom' : 'plugin');
      return;
    }
    if (event.key === 'Enter' && !event.shiftKey) {
      if (!isBalanced(input.value)) return;
      event.preventDefault();
      event.stopPropagation();
      submit();
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      hide();
    }
  }, true);
  input.addEventListener('input', resizeInput);
  layerButton.addEventListener('click', () => setLayer(state.layer === 'plugin' ? 'dom' : 'plugin'));
  clearButton.addEventListener('click', () => {
    state.lines = [];
    out.textContent = '';
    input.focus();
  });
  closeButton.addEventListener('click', hide);

  const show = () => {
    state.visible = true;
    host.style.display = 'block';
    input.focus();
    out.scrollTop = out.scrollHeight;
  };
  const hide = () => {
    state.visible = false;
    host.style.display = 'none';
  };
  const toggle = (next) => {
    const visible = next === undefined ? !state.visible : Boolean(next);
    if (visible) show();
    else hide();
  };
  const clear = () => {
    state.lines = [];
    out.textContent = '';
  };
  const feed = (text) => {
    append(text);
    out.scrollTop = out.scrollHeight;
  };
  const remove = () => {
    document.removeEventListener('keydown', onGlobalKey, true);
    host.remove();
    if (window.__coconutTerm === api) window.__coconutTerm = null;
  };
  const onGlobalKey = (event) => {
    if (!event.ctrlKey || event.altKey || event.metaKey) return;
    if (event.code !== 'KeyT' && event.key !== 't' && event.key !== 'T' && event.key !== '`' && event.key !== '~') return;
    event.preventDefault();
    event.stopPropagation();
    if (!event.repeat) toggle();
  };
  const api = {
    host,
    show,
    hide,
    toggle,
    clear,
    feed,
    remove,
    visible: () => state.visible,
    poll: () => JSON.stringify({ commands: state.commands.splice(0), visible: state.visible }),
  };
  document.addEventListener('keydown', onGlobalKey, true);
  document.body.appendChild(host);
  window.__coconutTerm = api;
  setLayer('plugin');
  return 'installed';
};

const removeTerminal = () => {
  if (window.__coconutTerm) window.__coconutTerm.remove();
  return true;
};

async function install() {
  try {
    await Coconut.runJs(pageCall(installTerminal));
    ready = true;
  } catch (error) {
    ready = false;
    if (!/页面还没打开|WS 连接已关闭|WS 发送失败/.test(String(error))) logError(error);
  }
}

const feed = (text) => {
  if (!ready) return;
  try {
    Coconut.runJs(pageCall(feedPage, text));
  } catch (error) {
    ready = false;
  }
};

const clear = () => {
  if (!ready) return;
  try {
    Coconut.runJs(pageCall(clearPage));
  } catch (error) {
    ready = false;
  }
};

const runDom = (code) => {
  const wrapped = `return (async () => { const __value = await (${code}); return typeof __value === 'string' ? __value : JSON.stringify(__value); })()`;
  try {
    feed(Coconut.runJs(wrapped));
  } catch (error) {
    try {
      feed(Coconut.runJs(`return (async () => { ${code}\n })()`));
    } catch (inner) {
      feed(String((inner && inner.message) || inner));
    }
  }
};

const runPlugin = (code) => {
  let value;
  try {
    value = eval(code);
  } catch (error) {
    feed(String((error && error.stack) || error));
    return;
  }
  Promise.resolve(value).then(
    (result) => feed(formatValue(result)),
    (error) => feed(String((error && error.stack) || error)),
  );
};

const runCommand = (command) => {
  const code = String((command && command.code) || '');
  if (!code.trim()) return;
  if (command.layer === 'dom') runDom(code);
  else runPlugin(code);
};

const pollConsole = () => {
  let response;
  try {
    response = Coconut.fetch(`${CONSOLE_ENDPOINT}?since=${cursor}`, {});
  } catch (error) {
    return;
  }
  const data = response && response.ok ? response.json : null;
  if (!data) return;
  if (data.cleared && data.cleared !== cleared) {
    cleared = data.cleared;
    cursor = Number(data.seq) || cursor;
    clear();
    return;
  }
  const lines = Array.isArray(data.lines) ? data.lines : [];
  if (!lines.length) return;
  const text = lines.map((line) => {
    if (line && line.seq) cursor = Math.max(cursor, Number(line.seq) || 0);
    const at = line && line.plugin ? '[' + line.plugin + '] ' : '';
    return at + (line.level === 'error' ? '✖ ' : line.level === 'warn' ? '! ' : '') + String(line.message || '');
  }).join('\n');
  feed(text);
};

const tick = async () => {
  if (ticking) return;
  ticking = true;
  try {
    if (!ready) await install();
    if (!ready) return;
    const raw = Coconut.runJs(pageCall(pollPage));
    const data = JSON.parse(raw);
    for (const command of data.commands || []) runCommand(command);
    pollConsole();
  } catch (error) {
    if (/页面还没打开|WS 连接已关闭|WS 发送失败/.test(String(error))) ready = false;
    else logError(error);
  } finally {
    ticking = false;
  }
};

async function onStart() {
  Coconut.log('调试终端已加载（' + Coconut.plugin.version + '）');
}

async function onHello() {
  await install();
  if (!pollTimer) {
    pollTimer = Coconut.setInterval(() => {
      void tick();
    }, 150);
  }
  await tick();
}

async function onStop() {
  if (pollTimer) {
    Coconut.clearInterval(pollTimer);
    pollTimer = null;
  }
  try {
    Coconut.runJs(pageCall(removeTerminal));
  } catch (error) {
    /* 页面可能已经关闭 */
  }
  ready = false;
}

globalThis.onStart = onStart;
globalThis.onHello = onHello;
globalThis.onStop = onStop;
