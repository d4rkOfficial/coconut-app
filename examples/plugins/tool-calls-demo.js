// @name        工具调用监视器
// @version     0.1.0
// @description 演示 Event / EventTarget / tools 异步调用 / AbortController / http / webSocket / rAF 的新 API
// @permissions toolbox, ui, dom, server, network
// @mount       overlay
//
// ⚠️ 设计预览：本文件用来确认 API 形状，Coconut.Event / EventTarget / tools / AbortController
//    目前**还没有实现**。原有的 Coconut.callTool 继续保留，不废弃。

// ── 三条通用约定（全部向 web 标准靠） ───────────────────────────────────────
//   1. 事件：{ type, detail, target, timeStamp, defaultPrevented }，dispatch 同步；
//      addEventListener(type, fn, { once, signal }) —— signal 可直接来自 AbortController。
//   2. 取消：一律 AbortController / AbortSignal，没有自定义的 .cancel()。
//   3. 失败：统一 reject 成 DOMException；被 abort 的是 name === 'AbortError'。

const stats = { calls: 0, failed: 0, aborted: 0, totalMs: 0, last: [] };

// ── 1. 事件的基础层：插件也能给自己建事件总线 ────────────────────────────────
const bus = new Coconut.EventTarget();

bus.addEventListener('tick', () => {
  paint();
});

function resetStats() {
  stats.calls = 0;
  stats.failed = 0;
  stats.aborted = 0;
  stats.totalMs = 0;
  stats.last = [];
  bus.dispatchEvent(new Coconut.Event('stats:reset', { detail: { at: Date.now() } }));
}

// ── 2. 异步调用：fetch 风格，第二个参数是 init ───────────────────────────────
//   Coconut.tools 是 EventTarget；调用方法挂在它上面，读作 "tools.call"。
//   返回 Promise（没有 handle、没有 id 要你记），取消走 signal。
const onToolResult = (event) => {
  const { tool, ms } = event.detail;
  stats.calls += 1;
  stats.totalMs += ms;
  stats.last.unshift({ tool, ms, ok: true });
  stats.last.length = Math.min(stats.last.length, 8);
  Coconut.log(`[tools] ${tool} 完成 ${ms}ms`);
  paint();
};

const onToolError = (event) => {
  const { tool, error, aborted } = event.detail;
  if (aborted) stats.aborted += 1;
  else stats.failed += 1;
  Coconut.log(`[tools] ${tool} ${aborted ? '已中止' : '失败'}：${error}`);
  paint();
};

Coconut.tools.addEventListener('result', onToolResult);
Coconut.tools.addEventListener('error', onToolError);

async function selfTest() {
  // 普通调用
  const result = await Coconut.tools.call('get_damage_entries', {});

  // 可中止：超时 / 用户取消 / 插件卸载，同一个 controller 能管住
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(new Error('自检超时')), 8000);
  try {
    await Coconut.tools.call('get_result_summary', { summaryOnly: true }, { signal: controller.signal });
  } catch (error) {
    // name === 'AbortError' 时 error.cause 就是传给 abort() 的 reason
    Coconut.log('[tools] 中止：' + error.name + ' / ' + error.message);
  } finally {
    clearTimeout(timer);
  }

  // AbortSignal.timeout() 也是标准的一部分，等价于上面那三行
  try {
    await Coconut.tools.call('get_result_summary', {}, { signal: AbortSignal.timeout(5000) });
  } catch (error) {
    if (error.name === 'TimeoutError') Coconut.log('[tools] 超时');
  }

  return result;
}

// 只关心某一个工具：注册监听时用 signal 管生命周期，不需要自己配对 remove
function whenTool(tool, fn) {
  const controller = new AbortController();
  Coconut.tools.addEventListener(
    'result',
    (event) => {
      if (event.detail.tool === tool) fn(event.detail.result, event.detail);
    },
    { signal: controller.signal },
  );
  return () => controller.abort(); // 注销：abort 之后监听器自动移除
}

const offSummary = whenTool('get_result_summary', (result) => {
  Coconut.log('[tools] 队总伤害：', JSON.stringify(result).slice(0, 120));
});
// 用完：offSummary()

// ── 3. 原有的 Coconut.callTool 不废弃，继续是一等公民 ────────────────────────
//   同步、直接拿结果、出错抛异常。适合短脚本和顺序编排；
//   代价是阻塞插件线程（最多 30s，期间定时器 / socket / http 事件都不派发）。
//   经验法则：一次调用就够用 → callTool；循环 / 并发 / 要进度 / 要能中止 → tools.call。
function quickSummary() {
  return Coconut.callTool('get_result_summary', { summaryOnly: true });
}

// ⚠️ 不要在 tools.call 的 await 期间再调 callTool —— 它会把泵一起堵住。

// ── 4. http：request 事件收到事件对象；老的双参写法仍然有效 ──────────────────
let httpServer = null;
let socket = null;

const onRequest = (event) => {
  const { request, response } = event.detail;
  if (request.path === '/stats') {
    response.status(200).json(stats);
    return;
  }
  response.status(404).send('not found');
};

const openHttp = () => {
  httpServer = Coconut.http({ host: '127.0.0.1', port: 0 }); // 0 = 随机端口
  httpServer.addEventListener('request', onRequest);
  // 兼容写法（fn.length >= 2 时引擎仍按双参调用）：
  //   httpServer.addEventListener('request', (req, res) => res.json(stats));
  Coconut.log(`[tools] http 监听 127.0.0.1:${httpServer.port}`);
};

// ── 5. webSocket：按 WebSocket 标准来 ───────────────────────────────────────
//   构造立刻返回，readyState = CONNECTING(0)，open 之后才是 OPEN(1)；
//   CONNECTING 阶段 send() 抛 InvalidStateError（不静默排队，和浏览器一致）。
const onSocketMessage = (event) => {
  Coconut.log('[tools] ws 收到：', event.data);
};

const openSocket = (url) => {
  if (!url) return;
  socket = Coconut.webSocket(url);
  if (socket.readyState !== Coconut.webSocket.OPEN) {
    Coconut.log('[tools] ws 连接中…');
  }
  socket.addEventListener('open', () => {
    Coconut.log('[tools] ws 已连接', url);
    socket.send(JSON.stringify({ hello: 'monitor' }));
  });
  socket.addEventListener('message', onSocketMessage);
  socket.addEventListener('close', (event) => {
    Coconut.log('[tools] ws 关闭', event.code, event.reason, 'wasClean=' + event.wasClean);
  });
  socket.addEventListener('error', (event) => Coconut.log('[tools] ws 错误：', event.message));
};

// ── 6. 浮层 + requestAnimationFrame ─────────────────────────────────────────
//   动画写在页面侧（原生 rAF，真正 60fps，零跨进程往返）：
//   overlay 的 script 在工具箱页面里执行一次，返回值作为销毁回调。
let overlayId = 0;
let rafId = 0;
let panelMounted = false;

const overlayScript = function () {
  const bar = document.querySelector('.tc-card .tc-bar');
  if (!bar) return null;
  let x = 0;
  let raf = 0;
  const tick = () => {
    x = (x + 1.2) % 100;
    bar.style.transform = 'translateX(' + x + '%)';
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(raf);   // 浮层移除时自动停，不用插件侧再管
};

let lastPaint = 0;

//   插件层用 Coconut.ui.requestAnimationFrame（不挂全局，免得和页面侧的原生 rAF 混淆）。
//   它是页面 rAF 合并后回传的：≤30Hz、1～2 帧抖动，只适合低频驱动。
//   权限：ui（渲染时钟，不碰 DOM）。
function startPluginLoop() {
  const tick = (timestamp) => {
    // 每 250ms 才真的重绘一次面板，其余帧只走一遍事件总线（空转很便宜）
    if (timestamp - lastPaint > 250) {
      lastPaint = timestamp;
      bus.dispatchEvent(new Coconut.Event('tick', { detail: { ts: timestamp } }));
    }
    rafId = Coconut.ui.requestAnimationFrame(tick);
  };
  rafId = Coconut.ui.requestAnimationFrame(tick);
}

// ── 7. 生命周期：hello 每个会话只来一次，页面没了会收到 bye ──────────────────
//   引擎按 session id 去重（4 秒重发 / last_hello 重放不会再触发 hello），
//   所以 onHello 可以放心地非幂等；重连是新会话，会正常再来一次。
async function onHello(info) {
  panelMounted = true;
  overlayId = Coconut.ui.overlay({
    html: '<div class="tc-card"><div class="tc-bar"></div><div class="tc-body"></div></div>',
    css: PANEL_CSS,
    script: overlayScript,
  });

  openHttp();
  openSocket(Coconut.storage.get('wsUrl', ''));
  selfTest().catch(() => {});
  try {
    quickSummary();
  } catch (error) {
    Coconut.log('[tools] quickSummary 失败：' + error.message);
  }
  startPluginLoop();
  paint();
}

async function onBye(info) {
  // 页面会话结束：浮层已经随页面消失，这里只收拾插件侧的东西
  Coconut.log('[tools] 会话结束', info && info.session);
  stopAll();
}

async function onStop() {
  stopAll();
}

function stopAll() {
  if (rafId) Coconut.ui.cancelAnimationFrame(rafId);
  rafId = 0;
  if (httpServer) {
    httpServer.removeEventListener('request', onRequest);
    httpServer.close();
    httpServer = null;
  }
  if (socket) {
    socket.removeEventListener('message', onSocketMessage);
    socket.close();
    socket = null;
  }
  Coconut.tools.removeEventListener('result', onToolResult);
  Coconut.tools.removeEventListener('error', onToolError);
  offSummary();
  if (overlayId) Coconut.ui.remove(overlayId);
  overlayId = 0;
  panelMounted = false;
}

// ── 渲染：浮层只读，内容由页面侧脚本填 ──────────────────────────────────────
const PANEL_CSS = `
  .tc-card {
    position: absolute; right: 20px; bottom: 20px; width: 240px;
    padding: 10px 14px; border-radius: 12px; overflow: hidden;
    background: rgba(20, 22, 26, .88); color: #e7ecf3;
    font: 12px/1.7 "Cascadia Mono", Consolas, monospace;
    box-shadow: 0 12px 30px rgba(0, 0, 0, .4); pointer-events: auto;
  }
  .tc-card b { color: #8fd3ff; font-weight: 600; }
  .tc-row { opacity: .72; }
  .tc-bar { position: absolute; left: 0; top: 0; height: 2px; width: 40%;
            background: linear-gradient(90deg, transparent, #8fd3ff); }
`;

function paint() {
  const avg = stats.calls ? Math.round(stats.totalMs / stats.calls) : 0;
  const body =
    '<b>工具调用</b><br>' +
    '<span class="tc-row">完成 ' + stats.calls + ' · 失败 ' + stats.failed +
    ' · 中止 ' + stats.aborted + ' · 平均 ' + avg + 'ms</span>';
  // 函数对象 + 参数：源码送到页面执行，参数走 JSON，不用手拼字符串
  try {
    Coconut.toolbox.runJs(function (html) {
      const el = document.querySelector('.tc-card .tc-body');
      if (el) el.innerHTML = html;
      return true;
    }, body);
  } catch (error) {
    Coconut.log('[tools] 面板落盘失败：' + error.message);
  }
}
