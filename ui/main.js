const $ = (id) => document.getElementById(id);
const t = (key, vars) => window.COCONUT_LANG.t(key, vars);

let state = null;
let connected = false;
let busy = false;
let toolboxUpdate = null;
let gameUpdate = null;
const resetTimers = new Map();

const RESULT_HOLD_MS = 3000;
const MIRROR_HOLD_MS = 1000;
const AUTO_EXIT_MS = 5 * 60 * 1000;
let statusHoldUntil = 0;
let mirrorHold = false;
let mirrorLinkEnabled = true;
let updatingToolbox = false;
let uninstallTimer = 0;
let exitTicker = 0;
let exitDeadline = 0;
let hasStatic = true;
let refreshing = false;
/** 前端发起的检查 / 更新：'check-toolbox' | 'update-toolbox' | 'check-game' | 'update-game'。 */
let activeTask = null;
/** 后端在跑自动检查 / 更新（例如启动 1.5 秒后的自动更新），由 get_state 轮询得知。 */
let backendActivity = false;
/** 用户刚点过停止：忽略后端残留的 activity，避免按钮又红回去。 */
let suppressActivity = false;
let starting = false;
/** 用户按过停止：启动时的自动检查是「工具箱 + 游戏数据」连跑，停一次就整串都停。 */
let autoSeriesStopped = false;

const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
let currentTheme = darkQuery.matches ? 'dark' : 'light';

function invoke(command, args) {
  if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
    return window.__TAURI__.core.invoke(command, args || {});
  }
  return Promise.reject(new Error('not in tauri'));
}

let themeLocked = false;

/** 渲染主题（不动 Rust 侧状态）。 */
function paintTheme(theme) {
  currentTheme = theme === 'dark' ? 'dark' : 'light';
  document.documentElement.dataset.theme = currentTheme;
  const label = document.getElementById('theme-label');
  if (label) label.textContent = currentTheme === 'dark' ? 'DARK' : 'LIGHT';
  const toggle = document.getElementById('theme-toggle');
  if (toggle) {
    toggle.setAttribute('aria-pressed', currentTheme === 'dark' ? 'true' : 'false');
    const key = currentTheme === 'dark' ? 'theme.toLight' : 'theme.toDark';
    toggle.title = t(key);
    toggle.setAttribute('aria-label', t(key));
    toggle.dataset.i18nTitle = key;
  }
}

/** 应用主题并同步给启动器（窗口标题栏 + 工具箱）。 */
function applyTheme(theme, options) {
  paintTheme(theme);
  invoke('set_window_theme', { theme: currentTheme }).catch(() => {});
  if (!options || options.persist !== false) {
    invoke('set_theme', { theme: currentTheme }).catch(() => {});
  }
}

/** 用户点了主题按钮：固定下来，不再跟随系统。 */
function toggleTheme() {
  themeLocked = true;
  applyTheme(currentTheme === 'dark' ? 'light' : 'dark');
}

function setStatus(text, kind, hold) {
  $('status').textContent = text;
  $('dot').className = 'status-dot' + (kind ? ' ' + kind : '');
  statusHoldUntil = hold ? Date.now() + RESULT_HOLD_MS : 0;
}

function setProgress(text, kind, options) {
  const line = String(text || '').replace(/\s+/g, ' ').trim();
  const node = $('progress-text');
  const track = $('progress-track');
  node.textContent = '';
  const label = document.createElement('span');
  label.className = 'progress-label';
  label.textContent = line || t('progress.idle');
  node.appendChild(label);
  node.title = line;
  node.className = 'progress-text' + (kind ? ' ' + kind : '');
  track.className =
    'progress' +
    (kind === 'busy' || kind === 'done' || kind === 'error' ? ' active' : '') +
    (kind === 'error' ? ' error' : '') +
    (kind === 'done' ? ' done' : '');

  if (options && options.mirror) {
    const link = document.createElement('button');
    link.type = 'button';
    link.id = 'mirror-link';
    link.className = 'mirror-link';
    link.textContent = t('mirror.hint');
    link.disabled = false;
    link.addEventListener('click', onMirrorClick);
    node.appendChild(link);
  }
}

async function onMirrorClick(event) {
  const link = event.currentTarget;
  if (!mirrorLinkEnabled || !updatingToolbox) return;
  mirrorLinkEnabled = false;
  mirrorHold = true;
  link.textContent = t('mirror.done');
  link.disabled = true;
  try {
    const result = await invoke('switch_mirror');
    const index = result && Number.isFinite(result.index) ? result.index : 0;
    const name = index > 0 ? t('mirror.n', { n: index }) : t('mirror.direct');
    setStatus(t('mirror.doneNamed', { label: name }), 'busy');
  } catch (error) {
    const detail = error?.message || String(error) || '';
    setStatus(t('mirror.failed', { detail }), 'error', true);
  }
  setTimeout(() => {
    mirrorLinkEnabled = true;
    mirrorHold = false;
    refresh();
  }, MIRROR_HOLD_MS);
}

function progressKind(text) {
  // 进度文案已经是按语言本地化过的，用多语言关键字判断「是不是错误」
  const failed = /失败|錯誤|错误|中断|中斷|不可用|无法|無法|エラー|失敗|실패|error|failed/i.test(text);
  return failed ? 'error' : busy ? 'busy' : 'done';
}

function setPhase(text) {
  $('phase').textContent = text;
}

/**
 * 标题按当前语言的字符宽度自适应：英文 / 日文标题比「椰果工具箱」长得多，
 * 直接用 clamp 的字号会换行、把下面的内容顶下去。这里量一遍，必要时缩小到一行。
 */
function fitTitle() {
  const title = document.querySelector('h1');
  if (!title) return;
  title.style.fontSize = '';
  title.style.whiteSpace = 'nowrap';
  const parent = title.parentElement;
  const available = (parent ? parent.clientWidth : window.innerWidth * 0.55) - 2;
  const range = document.createRange();
  const measure = () => {
    range.selectNodeContents(title);
    return range.getBoundingClientRect().width;
  };
  let size = parseFloat(getComputedStyle(title).fontSize) || 54;
  let guard = 0;
  while (measure() > available && size > 24 && guard < 80) {
    size = Math.max(24, size - 2);
    title.style.fontSize = size + 'px';
    guard += 1;
  }
}

/**
 * 让「鸣潮社区公益工具」通过字间距撑到 大标题文字宽度的 61.803%（黄金分割）。
 * 直接量 h1 的文字宽（Range 不含块级剩余空白），再按字符数解出字间距。
 */
function fitByline() {
  const byline = document.querySelector('.byline');
  const title = document.querySelector('h1');
  if (!byline || !title) return;

  const range = document.createRange();
  range.selectNodeContents(title);
  const titleWidth = range.getBoundingClientRect().width;
  const chars = Array.from((byline.textContent || '').trim()).length;
  if (!titleWidth || !chars) return;

  const target = titleWidth * 0.61803;
  byline.style.letterSpacing = '0px';
  const natural = byline.getBoundingClientRect().width;
  if (!natural) return;

  let spacing = (target - natural) / chars;
  byline.style.letterSpacing = spacing.toFixed(2) + 'px';

  // letter-spacing 的具体算法各浏览器略有差异，按实测再修一次。
  const actual = byline.getBoundingClientRect().width;
  if (Math.abs(actual - target) > 0.5) {
    spacing += (target - actual) / chars;
    byline.style.letterSpacing = spacing.toFixed(2) + 'px';
  }
}

function shortCommit(value) {
  return String(value || '').slice(0, 8);
}

function setButtonLabel(id, text) {
  const node = $(id);
  if (!node) return;
  // 按钮里除了文字还有图标：只替换文字节点，别用 textContent 把 SVG 一起清掉。
  const label = node.querySelector('[id$="-label"]');
  if (label) label.textContent = text;
  else node.textContent = text;
}

function scheduleReset(id, text) {
  const previous = resetTimers.get(id);
  if (previous) clearTimeout(previous);
  resetTimers.set(
    id,
    setTimeout(() => {
      if (busy) {
        scheduleReset(id, text);
        return;
      }
      setButtonLabel(id, text);
    }, RESULT_HOLD_MS),
  );
}

function isStopping() {
  return Boolean(activeTask) || backendActivity;
}

/** 启动按钮：空闲时是「启动工具箱」，检查 / 更新期间变成红色的「停止更新」。 */
function syncStartButton() {
  const button = $('start');
  if (!button) return;
  const stopping = isStopping();
  let label = t('start');
  if (starting) label = t('start.busy');
  else if (stopping) label = t('start.stop');
  $('start-label').textContent = label;
  button.classList.toggle('stopping', stopping && !starting);
  button.dataset.mode = stopping ? 'stop' : 'start';
  button.title = stopping ? t('start.stopTitle') : '';
  button.disabled = !connected || starting || (!hasStatic && !stopping);
}

function disableActions(disabled) {
  // 检查 / 更新 / 自动更新期间，「启动工具箱」变成「停止更新」而不是禁用：
  // 点它就把当前的检查或更新停下来。游戏数据在后台拉取时也是同样处理。
  $('check-toolbox').disabled = disabled || !connected;
  $('check-game').disabled = disabled || !connected;
  $('uninstall').disabled = disabled;
  $('plugins').disabled = !connected;
  syncStartButton();
}

// ── 插件管理 ──
// 顶部标题栏（右上角关闭）+ 搜索 / 状态筛选 / 批量启停；列表是紧凑行，
// 每行右侧是启用开关 + 重载。停用/启停都是乐观更新：先改本地状态立刻重绘，再落 Rust。
let pluginEntries = [];
let pluginFilter = 'all';
let pluginQuery = '';
let pluginBusy = false;

function setPluginBadge(list) {
  const button = $('plugins');
  if (!button) return;
  const errored = list.some((plugin) => plugin.errors > 0);
  const enabled = list.filter((plugin) => plugin.enabled).length;
  // 文案固定「插件」，状态只靠小圆点：默认黄灯，有插件报错才变红灯。
  button.dataset.error = errored ? 'true' : 'false';
  $('plugins-label').textContent = t('plugins.button');
  button.title = t(errored ? 'plugins.badgeTipError' : 'plugins.badgeTip', {
    count: list.length,
    enabled,
  });
}

function pluginMatchesQuery(plugin) {
  if (!pluginQuery) return true;
  const haystack = [
    plugin.name,
    plugin.id,
    plugin.description,
    plugin.path,
    ...(plugin.permissions || []),
  ]
    .map((value) => String(value || '').toLowerCase())
    .join('\n');
  return haystack.includes(pluginQuery);
}

function pluginPassesFilter(plugin) {
  if (pluginFilter === 'on') return plugin.enabled;
  if (pluginFilter === 'off') return !plugin.enabled;
  if (pluginFilter === 'error') return plugin.errors > 0;
  return true;
}

/** 标题栏计数、分段按钮上的数字、批量按钮的可用性。 */
function updatePluginChrome(list) {
  const total = list.length;
  const enabled = list.filter((plugin) => plugin.enabled).length;
  const errored = list.filter((plugin) => plugin.errors > 0).length;

  const count = $('plugins-count');
  if (count) {
    count.textContent = total ? t('plugins.countLine', { total, enabled }) : '—';
    count.dataset.error = errored > 0 ? 'true' : 'false';
    count.title = t('plugins.countTip', { total, enabled, errored });
  }

  const counts = { all: total, on: enabled, off: total - enabled, error: errored };
  const labels = {
    all: 'plugins.filter.all',
    on: 'plugins.filter.on',
    off: 'plugins.filter.off',
    error: 'plugins.filter.error',
  };
  const filters = $('plugins-filters');
  if (filters) {
    for (const button of filters.querySelectorAll('button[data-filter]')) {
      const key = button.dataset.filter;
      // 文案不挂 data-i18n：静态替换会连数量一起抹掉，这里统一重建「标签 + 数量」
      button.innerHTML = `<span>${escapeHtml(t(labels[key] || labels.all))}</span><b>${
        counts[key] ?? 0
      }</b>`;
      button.setAttribute('aria-pressed', key === pluginFilter ? 'true' : 'false');
    }
  }

  // 全部启用 / 停用合并为一个按钮：按当前状态显示相反动作
  const toggleAll = $('plugins-toggle-all');
  if (toggleAll) {
    toggleAll.disabled = pluginBusy || total === 0;
    const anyOn = enabled > 0;
    toggleAll.textContent = t(anyOn ? 'plugins.disableAll' : 'plugins.enableAll');
    toggleAll.dataset.action = anyOn ? 'off' : 'on';
  }
}

function pluginPermChips(permissions) {
  return (permissions || [])
    .map(
      (perm) =>
        `<span class="plugin-perm${perm === 'debug' ? ' debug' : ''}">${escapeHtml(perm)}</span>`,
    )
    .join('');
}

function pluginRowHtml(plugin) {
  const badges = [
    plugin.builtin
      ? `<span class="plugin-badge builtin">${escapeHtml(t('plugins.builtin'))}</span>`
      : '',
    plugin.mount === 'overlay'
      ? `<span class="plugin-badge">${escapeHtml(t('plugins.mountOverlay'))}</span>`
      : '',
    plugin.errors > 0
      ? `<span class="plugin-badge error">${escapeHtml(
          t('plugins.errorBadge', { count: plugin.errors }),
        )}</span>`
      : '',
  ]
    .filter(Boolean)
    .join('');
  const description = plugin.description
    ? `<div class="plugin-desc" title="${escapeHtml(plugin.description)}">${escapeHtml(
        plugin.description,
      )}</div>`
    : '';
  const error = plugin.errors > 0
    ? `<div class="plugin-error" title="${escapeHtml(plugin.last_error || '')}">${escapeHtml(
        t('plugins.errorLine', { count: plugin.errors, message: plugin.last_error || '' }),
      )}</div>`
    : '';
  const label = t(plugin.enabled ? 'plugins.disable' : 'plugins.enable');
  return `<li class="plugin-row" data-enabled="${plugin.enabled}" data-error="${
    plugin.errors > 0
  }" data-id="${escapeHtml(plugin.id)}">
      <span class="plugin-dot" aria-hidden="true"></span>
      <div class="plugin-main">
        <div class="plugin-top">
          <span class="plugin-name" title="${escapeHtml(plugin.path || plugin.id)}">${escapeHtml(
            plugin.name || plugin.id,
          )}</span>
          <span class="plugin-version">v${escapeHtml(plugin.version || '0.0.0')}</span>
          ${badges}
        </div>
        ${description}
        <div class="plugin-meta">
          ${pluginPermChips(plugin.permissions)}
          <span class="plugin-id">${escapeHtml(plugin.id)}</span>
        </div>
        ${error}
      </div>
      <div class="plugin-side">
        <label class="plugin-switch" title="${escapeHtml(label)}">
          <input
            type="checkbox"
            data-act="toggle"
            ${plugin.enabled ? 'checked' : ''}
            aria-label="${escapeHtml(label)}"
          />
          <span class="plugin-slider" aria-hidden="true"></span>
        </label>
        <button class="plugin-action" type="button" data-act="reload">${escapeHtml(
          t('plugins.reload'),
        )}</button>
      </div>
    </li>`;
}

function renderPlugins() {
  const list = pluginEntries;
  setPluginBadge(list);
  updatePluginChrome(list);
  const root = $('plugins-list');
  const blank = $('plugins-blank');
  if (!root || !blank) return;
  root.innerHTML = '';

  if (!list.length) {
    blank.textContent = t('plugins.empty');
    blank.hidden = false;
    return;
  }

  const visible = list.filter(
    (plugin) => pluginPassesFilter(plugin) && pluginMatchesQuery(plugin),
  );
  if (!visible.length) {
    blank.textContent = pluginQuery
      ? t('plugins.emptyQuery', { query: pluginQuery })
      : t('plugins.emptyFiltered');
    blank.hidden = false;
    return;
  }

  blank.hidden = true;
  root.innerHTML = visible.map(pluginRowHtml).join('');

  for (const row of root.querySelectorAll('.plugin-row')) {
    const plugin = list.find((item) => item.id === row.dataset.id);
    if (!plugin) continue;
    row
      .querySelector('input[data-act="toggle"]')
      ?.addEventListener('change', () => onPluginAction(plugin, 'toggle'));
    row
      .querySelector('button[data-act="reload"]')
      ?.addEventListener('click', () => onPluginAction(plugin, 'reload'));
  }
}

function escapeHtml(text) {
  return String(text == null ? '' : text)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

async function loadPlugins() {
  try {
    const list = await invoke('plugins_list');
    pluginEntries = Array.isArray(list) ? list : [];
    renderPlugins();
  } catch (error) {
    setStatus(t('plugins.listFailed', { detail: error?.message || error }), 'error', true);
  }
}

async function onPluginAction(plugin, action) {
  if (pluginBusy) return;
  pluginBusy = true;
  updatePluginChrome(pluginEntries);
  try {
    if (action === 'toggle') {
      const next = !plugin.enabled;
      // 乐观更新：先本地切、立刻重绘，失败再回滚
      plugin.enabled = next;
      renderPlugins();
      try {
        await invoke('plugins_set_enabled', { id: plugin.id, enabled: next });
        setStatus(
          t(next ? 'plugins.enableToast' : 'plugins.disableToast', { name: plugin.name }),
          'ready',
          true,
        );
      } catch (error) {
        plugin.enabled = !next;
        renderPlugins();
        throw error;
      }
    } else if (action === 'reload') {
      await invoke('plugins_reload', { id: plugin.id });
      setStatus(t('plugins.reloadToast', { name: plugin.name }), 'ready', true);
    }
  } catch (error) {
    setStatus(t('plugins.actionFailed', { detail: error?.message || error }), 'error', true);
  }
  pluginBusy = false;
  await loadPlugins();
}

/** 批量启用 / 停用：只处理需要变的那部分，逐个落 Rust（失败计数）。 */
async function bulkPluginAction(enabled) {
  if (pluginBusy) return;
  const targets = pluginEntries.filter((plugin) => plugin.enabled !== enabled);
  if (!targets.length) return;
  pluginBusy = true;
  updatePluginChrome(pluginEntries);
  let failed = 0;
  for (const plugin of targets) {
    try {
      await invoke('plugins_set_enabled', { id: plugin.id, enabled });
      plugin.enabled = enabled;
    } catch {
      failed += 1;
    }
  }
  pluginBusy = false;
  if (failed) {
    setStatus(t('plugins.bulkFailed', { count: failed }), 'error', true);
  } else {
    setStatus(
      t(enabled ? 'plugins.enableAllToast' : 'plugins.disableAllToast', {
        count: targets.length,
      }),
      'ready',
      true,
    );
  }
  await loadPlugins();
}

/** 清理已从目录删除的插件留下的 state / storage 残留。 */
async function cleanupDeadPlugins(options) {
  if (pluginBusy) return;
  const silent = Boolean(options && options.silent);
  pluginBusy = true;
  updatePluginChrome(pluginEntries);
  try {
    const result = await invoke('plugins_cleanup_dead');
    const states = Number(result?.states || 0);
    const storage = Number(result?.storage || 0);
    if (!silent) {
      if (!states && !storage) {
        setStatus(t('plugins.cleanupEmpty'), 'ready', true);
      } else {
        setStatus(t('plugins.cleanupToast', { states, storage }), 'ready', true);
      }
    }
  } catch (error) {
    if (!silent) setStatus(t('plugins.cleanupFailed', { detail: error?.message || error }), 'error', true);
  }
  pluginBusy = false;
  await loadPlugins();
}

let pluginScanTimer = 0;

function closePlugins() {
  $('plugins-dialog').hidden = true;
  if (pluginScanTimer) {
    clearInterval(pluginScanTimer);
    pluginScanTimer = 0;
  }
}

async function openPlugins() {
  if (!connected) return;
  $('plugins-dialog').hidden = false;
  // 打开时自动清理死数据（后台静默，不打断操作）
  cleanupDeadPlugins({ silent: true });
  await loadPlugins();
  const search = $('plugins-search');
  if (search) {
    search.value = pluginQuery;
    search.focus();
  }
  // 弹窗打开期间每 10 秒自动扫描并重载一次
  if (pluginScanTimer) clearInterval(pluginScanTimer);
  pluginScanTimer = setInterval(() => {
    if ($('plugins-dialog').hidden) {
      clearInterval(pluginScanTimer);
      pluginScanTimer = 0;
      return;
    }
    loadPlugins();
  }, 10000);
}

// ── 安装插件（本地文件 / 远程链接 / 拖拽）──
let installing = false;

function toggleInstallDrawer(force) {
  const dialog = $('install-dialog');
  const next = typeof force === 'boolean' ? force : dialog.hidden;
  dialog.hidden = !next;
  if (next) {
    // 默认回到"本地文件"tab
    switchInstallTab('file');
  }
}

function switchInstallTab(which) {
  for (const btn of document.querySelectorAll('#addtabs button')) {
    btn.classList.toggle('on', btn.dataset.tab === which);
  }
  $('addview-file').hidden = which !== 'file';
  $('addview-url').hidden = which !== 'url';
}

function closeInstall() {
  $('install-dialog').hidden = true;
  $('install-dialog').dataset.drag = 'false';
}

async function installPaths(paths) {
  if (installing || !paths.length) return;
  installing = true;
  let installed = 0;
  try {
    for (const path of paths) {
      const info = await invoke('plugins_install_file', { path });
      installed += 1;
      setStatus(
        t('plugins.installOk', { name: info.name || info.id, version: info.version || '0.0.0' }),
        'ready',
        true,
      );
    }
    if (installed > 0) await loadPlugins();
  } catch (error) {
    setStatus(t('plugins.installFailed', { detail: error?.message || error }), 'error', true);
    await loadPlugins();
  } finally {
    installing = false;
  }
}

async function pickAndInstallFile() {
  try {
    const selected = await invoke('plugin:dialog|open', {
      options: {
        multiple: true,
        title: t('plugins.installFile'),
        filters: [{ name: 'JavaScript', extensions: ['js'] }],
      },
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    await installPaths(paths.filter((path) => /\.js$/i.test(String(path || ''))));
  } catch (error) {
    setStatus(t('plugins.installFailed', { detail: error?.message || error }), 'error', true);
  }
}

async function installFromUrl() {
  if (installing) return;
  const input = $('plugins-url');
  const url = String(input?.value || '').trim();
  if (!url) {
    input?.focus();
    return;
  }
  installing = true;
  const button = $('plugins-url-go');
  const original = button.textContent;
  button.disabled = true;
  input.disabled = true;
  button.textContent = t('plugins.installBusy');
  try {
    const info = await invoke('plugins_install_url', { url });
    setStatus(
      t('plugins.installOk', { name: info.name || info.id, version: info.version || '0.0.0' }),
      'ready',
      true,
    );
    input.value = '';
    await loadPlugins();
  } catch (error) {
    setStatus(t('plugins.installFailed', { detail: error?.message || error }), 'error', true);
  } finally {
    installing = false;
    button.disabled = false;
    input.disabled = false;
    button.textContent = original;
    input.focus();
  }
}

/** WebView 拖拽 .js 文件进弹窗直接安装（工具箱页面打开时没有启动器弹窗，自动忽略）。 */
function initPluginDrop() {
  try {
    const webview = window.__TAURI__?.webview?.getCurrentWebview?.();
    if (!webview || typeof webview.onDragDropEvent !== 'function') return;
    webview.onDragDropEvent((event) => {
      const installDialog = $('install-dialog');
      const pluginsDialog = $('plugins-dialog');
      const active = !installDialog.hidden ? installDialog : !pluginsDialog.hidden ? pluginsDialog : null;
      if (!active) return;
      const payload = event.payload || {};
      if (payload.type === 'enter' || payload.type === 'over') {
        active.dataset.drag = 'true';
        return;
      }
      if (active === installDialog) installDialog.dataset.drag = 'false';
      else pluginsDialog.dataset.drag = 'false';
      if (payload.type !== 'drop') return;
      const paths = (payload.paths || []).filter((path) => /\.js$/i.test(String(path || '')));
      if (!paths.length) {
        setStatus(t('plugins.installInvalid'), 'error', true);
        return;
      }
      installPaths(paths).then(() => closeInstall());
    });
  } catch (error) {
    // 老运行时没有 drag-drop API：拖拽不可用，文件 / 链接安装不受影响。
  }
}

function initPlugins() {
  $('plugins').addEventListener('click', openPlugins);
  $('plugins-close').addEventListener('click', closePlugins);
  $('plugins-dialog').addEventListener('click', (event) => {
    if (event.target === $('plugins-dialog')) closePlugins();
  });

  $('plugins-open').addEventListener('click', async () => {
    try {
      const dir = await invoke('plugins_open_dir');
      setStatus(t('plugins.dirToast', { dir }), 'ready', true);
    } catch (error) {
      setStatus(t('plugins.openFailed', { detail: error?.message || error }), 'error', true);
    }
  });

  // 合并的「全部启用 / 停用」按钮：按当前状态决定动作
  $('plugins-toggle-all').addEventListener('click', (event) => {
    const btn = event.currentTarget;
    bulkPluginAction(btn.dataset.action !== 'off');
  });

  // 添加插件：独立弹窗
  $('plugins-add').addEventListener('click', () => toggleInstallDrawer(true));
  $('install-close').addEventListener('click', closeInstall);
  $('install-dialog').addEventListener('click', (event) => {
    if (event.target === $('install-dialog')) closeInstall();
  });
  for (const tab of document.querySelectorAll('#addtabs button')) {
    tab.addEventListener('click', () => switchInstallTab(tab.dataset.tab));
  }
  // 本地文件选择：走 Tauri 系统对话框（与拖拽一致，兼容性最好）
  const fileInput = $('install-file-input');
  const dropzone = $('install-dropzone');
  dropzone.addEventListener('click', () => {
    pickAndInstallFile().then(() => closeInstall());
  });
  // <input> 仅作为无 Tauri 环境（预览）时的兜底
  fileInput.addEventListener('change', () => {
    const names = Array.from(fileInput.files || []).map((f) => f.name).filter((n) => /\.js$/i.test(n));
    if (names.length) {
      setStatus(t('plugins.installOk', { name: names.join(', '), version: '—' }), 'ready', true);
      closeInstall();
    }
    fileInput.value = '';
  });
  // 远程链接安装
  $('plugins-url-go').addEventListener('click', installFromUrl);
  $('plugins-url').addEventListener('keydown', (event) => {
    if (event.key === 'Enter') installFromUrl();
  });

  // ESC：优先关添加插件弹窗，其次关插件弹窗
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    if (!$('install-dialog').hidden) closeInstall();
    else if (!$('plugins-dialog').hidden) closePlugins();
  });

  $('plugins-search').addEventListener('input', (event) => {
    pluginQuery = String(event.target.value || '').trim().toLowerCase();
    renderPlugins();
  });

  $('plugins-filters').addEventListener('click', (event) => {
    const button = event.target.closest('button[data-filter]');
    if (!button) return;
    pluginFilter = button.dataset.filter || 'all';
    updatePluginChrome(pluginEntries);
    renderPlugins();
  });

  initPluginDrop();
}

function applyState(next) {
  state = next;
  connected = Boolean(next.ready) && !next.error;
  $('commit').textContent = next.commit || '—';
  $('build').textContent = next.build || '—';
  $('game-version').textContent = next.game_version || '—';
  if (typeof next.has_static === 'boolean') {
    hasStatic = next.has_static;
  }
  // 后端自动检查 / 更新（启动 1.5 秒后那次）也把启动按钮切成「停止更新」。
  // 用户点过停止后先压住残留的 activity，等它真的归零再恢复正常轮询。
  if (suppressActivity && !next.activity) suppressActivity = false;
  backendActivity = Boolean(next.activity) && !suppressActivity;
  if (next.progress && !mirrorHold) {
    setProgress(next.progress, progressKind(next.progress), {
      mirror: connected && updatingToolbox,
    });
  }

  if (!connected) {
    setPhase('OFFLINE');
    if (!busy) setStatus(next.error || t('status.offline'), 'error');
    disableActions(true);
    return;
  }

  if (!busy && Date.now() >= statusHoldUntil && !backendActivity) {
    setPhase('READY');
    if (hasStatic) {
      setStatus(t('status.ready'), 'ready');
    } else {
      setStatus(t('status.missingStatic'), 'error');
    }
  }
  if (backendActivity && !busy) {
    setPhase(next.updating ? 'UPDATE' : 'CHECK');
    setStatus(
      next.updating ? t('status.autoUpdating') : t('status.autoChecking'),
      'busy',
    );
  }
  disableActions(busy);
}

async function refresh() {
  try {
    const next = await invoke('get_state');
    applyState(next);
  } catch (error) {
    connected = false;
    $('commit').textContent = '—';
    $('build').textContent = '—';
    $('game-version').textContent = '—';
    setPhase('OFFLINE');
    if (!busy) setStatus(error?.message || t('status.waitingService'), 'error');
    setProgress(t('progress.noService'), 'error');
    disableActions(true);
  }
}

async function runCheck(kind) {
  if (busy || !connected) return;
  if (kind === 'toolbox' && toolboxUpdate) {
    await runUpdate('toolbox');
    return;
  }
  if (kind === 'game' && gameUpdate) {
    await runUpdate('game');
    return;
  }

  const isToolbox = kind === 'toolbox';
  busy = true;
  activeTask = isToolbox ? 'check-toolbox' : 'check-game';
  suppressActivity = false;
  disableActions(true);
  const button = isToolbox ? 'check-toolbox' : 'check-game';
  const label = isToolbox ? t('check.toolbox') : t('check.game');
  statusHoldUntil = 0;
  // 检查版本也要能换镜像：清单同样是 直连 / 镜像 轮换拉取的。
  updatingToolbox = isToolbox;
  $(button).classList.add('neon-busy');
  setButtonLabel(button, isToolbox ? t('check.busyToolbox') : t('check.busyGame'));
  setProgress(
    isToolbox ? t('progress.connectingGithub') : t('progress.connectingGame'),
    'busy',
  );
  setPhase('CHECK');
  setStatus(isToolbox ? t('check.checkingToolbox') : t('check.checkingGame'), 'busy');

  try {
    const result = isToolbox
      ? await invoke('check_update')
      : await invoke('check_game_update');

    if (result.available) {
      if (isToolbox) {
        toolboxUpdate = result;
        setButtonLabel(button, t('check.updateTo', { value: shortCommit(result.remote) }));
        setStatus(t('check.foundToolbox', { value: shortCommit(result.remote) }), 'ready');
      } else {
        gameUpdate = result;
        setButtonLabel(button, t('check.updateToGame', { value: result.remote || '?' }));
        setStatus(t('check.foundGame', { value: result.remote || '?' }), 'ready');
      }
      setPhase('READY');
    } else {
      if (isToolbox) toolboxUpdate = null;
      else gameUpdate = null;
      setButtonLabel(
        button,
        isToolbox ? t('check.resultLatestToolbox') : t('check.resultLatestGame'),
      );
      setStatus(
        isToolbox
          ? t('check.latestToolbox', { value: shortCommit(result.local) })
          : t('check.latestGame', { value: result.local || '?' }),
        'ready',
      );
      setPhase('READY');
      scheduleReset(button, label);
    }
  } catch (error) {
    if (isToolbox) toolboxUpdate = null;
    else gameUpdate = null;
    if (isCancelledError(error)) {
      setButtonLabel(button, label);
      setStatus(t('update.stopped'), 'ready', true);
      statusHoldUntil = Date.now() + RESULT_HOLD_MS;
      setProgress(t('progress.stopped'), 'done');
      setPhase('READY');
    } else {
      const detail = error?.message || String(error) || '';
      setButtonLabel(button, t('check.failed'));
      setStatus(detail || t('check.failed'), 'error', true);
      setProgress(t('progress.checkFailed', { detail }), 'error');
      setPhase('READY');
      scheduleReset(button, label);
    }
  } finally {
    updatingToolbox = false;
    activeTask = null;
    busy = false;
    disableActions(false);
    $(button).classList.remove('neon-busy');
  }
}

async function runUpdate(kind) {
  if (busy || !connected) return;
  const isToolbox = kind === 'toolbox';
  // 拉取游戏数据是可以随时停止的后台任务；停止后仍可立刻启动工具箱。
  refreshing = !isToolbox;
  busy = true;
  activeTask = isToolbox ? 'update-toolbox' : 'update-game';
  suppressActivity = false;
  disableActions(true);
  const button = isToolbox ? 'check-toolbox' : 'check-game';
  const label = isToolbox ? t('check.toolbox') : t('check.game');
  statusHoldUntil = 0;
  updatingToolbox = isToolbox;
  $(button).classList.add('neon-busy');
  setButtonLabel(button, isToolbox ? t('update.busyToolbox') : t('update.busyGame'));
  setProgress(
    isToolbox ? t('update.preparingToolbox') : t('update.preparingGame'),
    'busy',
  );
  setPhase('UPDATE');
  setStatus(isToolbox ? t('update.runningToolbox') : t('update.runningGame'), 'busy');

  try {
    if (isToolbox) {
      await invoke('update_toolbox');
      toolboxUpdate = null;
    } else {
      await invoke('update_game_data');
      gameUpdate = null;
    }
    await refresh();
    setButtonLabel(
      button,
      isToolbox ? t('check.resultLatestToolbox') : t('check.resultLatestGame'),
    );
    setStatus(isToolbox ? t('update.doneToolbox') : t('update.doneGame'), 'ready');
    setProgress(
      isToolbox ? t('update.finishedToolbox') : t('update.finishedGame'),
      'done',
    );
    setPhase('READY');
    scheduleReset(button, label);
  } catch (error) {
    if (isToolbox) toolboxUpdate = null;
    else gameUpdate = null;
    if (isCancelledError(error)) {
      setButtonLabel(button, label);
      setStatus(t('update.stopped'), 'ready', true);
      statusHoldUntil = Date.now() + RESULT_HOLD_MS;
      setProgress(t('progress.stopped'), 'done');
      setPhase('READY');
    } else {
      const detail = error?.message || String(error) || '';
      setButtonLabel(button, t('check.failed'));
      setStatus(detail || t('check.failed'), 'error', true);
      setProgress(t('progress.updateFailed', { detail }), 'error');
      setPhase('READY');
      scheduleReset(button, label);
    }
  } finally {
    updatingToolbox = false;
    refreshing = false;
    activeTask = null;
    busy = false;
    disableActions(false);
    $(button).classList.remove('neon-busy');
  }
}

/** 后端用 `__cancelled__` 表示用户点了「停止更新」，不算失败。 */
function isCancelledError(error) {
  return (error?.message || String(error || '')) === '__cancelled__';
}

/** 启动按钮在检查 / 更新期间就是停止按钮：点它立刻全停，并马上恢复「启动工具箱」。 */
function stopUpdates() {
  // 一次性全停：当前前端任务、后端自动（检查 / 更新）、后续自动检查与更新全部放弃。
  // 不显示「正在停止」过渡态，也不等后端确认——按钮立即回到「启动工具箱」。
  autoSeriesStopped = true;
  suppressActivity = true;
  activeTask = null;
  backendActivity = false;
  busy = false;
  refreshing = false;
  starting = false;
  statusHoldUntil = Date.now() + RESULT_HOLD_MS;
  disableActions(false);
  setStatus(t('update.stopped'), 'ready', true);
  setProgress(t('progress.stopped'), 'done');
  setPhase('READY');
  // 后端收到令牌后会丢弃正在进行的连接 / 下载；即使晚一点收尾，界面也不再回到停止态。
  invoke('cancel_update').catch(() => {});
}

async function onStartClick() {
  if (isStopping()) {
    stopUpdates();
    return;
  }
  await startFlow();
}

async function startFlow() {
  if (!hasStatic) {
    setStatus(t('status.missingStaticStart'), 'error', true);
    return;
  }
  // 检查 / 更新期间按钮是「停止更新」，走到这里的只会是空闲状态。
  if (busy || isStopping() || !connected || !state?.url) return;
  starting = true;
  busy = true;
  disableActions(true);
  setPhase('START');
  setStatus(t('status.entering'), 'busy');
  setProgress(t('progress.openingToolbox'), 'busy');
  try {
    await invoke('open_toolbox_window', { url: state.url });
  } catch (error) {
    starting = false;
    busy = false;
    disableActions(false);
    setPhase('READY');
    const detail = error?.message || String(error) || '';
    setStatus(t('start.failed', { detail }), 'error', true);
    setProgress(t('progress.startFailed', { detail }), 'error');
  }
}

function openUninstallDialog() {
  const dialog = $('uninstall-dialog');
  if (!dialog || !dialog.hidden) return;
  $('uninstall-steps').textContent = '';
  $('uninstall-foot').textContent = t('uninstall.ready');
  $('uninstall-bar').style.width = '0%';
  $('uninstall-actions').hidden = true;
  const primary = $('uninstall-exit');
  primary.dataset.mode = 'exit';
  primary.textContent = t('uninstall.exit');
  $('uninstall-exit-app').hidden = true;
  dialog.hidden = false;
}

function buildUninstallSteps(steps) {
  const list = $('uninstall-steps');
  list.textContent = '';
  for (const step of steps) {
    const item = document.createElement('li');
    item.className = 'uninstall-step pending';
    item.dataset.key = step.key;
    const tick = document.createElement('span');
    tick.className = 'tick';
    const label = document.createElement('span');
    label.className = 'label';
    label.textContent = step.label;
    const detail = document.createElement('span');
    detail.className = 'detail';
    detail.textContent = step.detail;
    label.appendChild(detail);
    item.appendChild(tick);
    item.appendChild(label);
    list.appendChild(item);
  }
}

function renderUninstall(progress) {
  const steps = Array.isArray(progress.steps) ? progress.steps : [];
  if (!steps.length) return;
  const list = $('uninstall-steps');
  if (list.childElementCount !== steps.length) buildUninstallSteps(steps);
  steps.forEach((step, index) => {
    const node = list.children[index];
    if (node) node.className = 'uninstall-step ' + step.status;
  });
  const finished = steps.filter((step) => step.status === 'done').length;
  const failed = steps.some((step) => step.status === 'failed');
  const percent = Math.round((finished / steps.length) * 100);
  $('uninstall-bar').style.width = percent + '%';
  if (progress.error) {
    $('uninstall-foot').textContent = t('uninstall.error', { detail: progress.error });
  } else if (progress.done) {
    const installed = Boolean(state && state.uninstaller_dir);
    $('uninstall-actions').hidden = false;
    const primary = $('uninstall-exit');
    const secondary = $('uninstall-exit-app');
    if (installed) {
      primary.dataset.mode = 'installer';
      primary.textContent = t('uninstall.openInstallDir');
      secondary.hidden = false;
      secondary.textContent = t('uninstall.exit');
      if (!exitDeadline) $('uninstall-foot').textContent = t('uninstall.completedInstalled');
    } else {
      primary.dataset.mode = 'exit';
      primary.textContent = t('uninstall.exit');
      secondary.hidden = true;
      if (!exitDeadline) $('uninstall-foot').textContent = t('uninstall.completed');
    }
  } else {
    $('uninstall-foot').textContent = t('uninstall.stepProgress', {
      current: Math.min(finished + 1, steps.length),
      total: steps.length,
      failed: failed ? t('uninstall.stepFailed') : '',
    });
  }
}

/** 卸载完成后不再自动秒退：等用户点退出，或 5 分钟后静默退出。
 *  MSI 安装版需要用户手动运行 uninstall.exe，因此不自动退出。 */
function startExitCountdown(auto = true) {
  if (!auto || exitDeadline) return;
  exitDeadline = Date.now() + AUTO_EXIT_MS;
  const paint = () => {
    const left = Math.max(0, exitDeadline - Date.now());
    if (left <= 0) {
      $('uninstall-foot').textContent = t('uninstall.exiting');
      clearInterval(exitTicker);
      return;
    }
    const minutes = Math.floor(left / 60000);
    const seconds = String(Math.floor((left % 60000) / 1000)).padStart(2, '0');
    $('uninstall-foot').textContent = t('uninstall.countdown', {
      left: minutes + ':' + seconds,
    });
  };
  paint();
  exitTicker = setInterval(paint, 1000);
}

async function runUninstall() {
  if (busy) return;
  busy = true;
  disableActions(true);
  openUninstallDialog();
  setPhase('CLEAN');
  setStatus(t('status.uninstalling'), 'busy');

  const tick = async () => {
    try {
      const progress = await invoke('get_uninstall_state');
      renderUninstall(progress);
      if (progress.done) {
        clearInterval(uninstallTimer);
        uninstallTimer = 0;
        startExitCountdown(!(state && state.uninstaller_dir));
        setStatus(t('status.uninstalled'), 'ready');
      }
    } catch (error) {
      // 进程正在退出，invoke 失败是预期行为。
    }
  };

  try {
    await invoke('start_uninstall');
    await tick();
    clearInterval(uninstallTimer);
    uninstallTimer = setInterval(tick, 140);
  } catch (error) {
    const detail = error?.message || String(error) || '';
    $('uninstall-foot').textContent = t('uninstall.startFailed', { detail });
    busy = false;
    disableActions(false);
    setPhase('READY');
    setStatus(t('status.uninstallFailed', { detail }), 'error', true);
  }
}

async function exitLauncher(button) {
  if (!button || button.disabled) return;
  const label = button.textContent;
  button.disabled = true;
  button.textContent = t('uninstall.exiting');
  try {
    await invoke('exit_app');
  } catch (error) {
    button.disabled = false;
    button.textContent = label;
  }
}

async function handleUninstallPrimary() {
  const button = $('uninstall-exit');
  if (button.dataset.mode !== 'installer') {
    await exitLauncher(button);
    return;
  }
  if (button.disabled) return;
  button.disabled = true;
  try {
    const dir = await invoke('open_uninstaller_dir');
    $('uninstall-foot').textContent = t('uninstall.openedDir', { dir });
    setStatus(t('uninstall.openedDir', { dir }), 'ready', true);
  } catch (error) {
    const detail = error?.message || String(error) || '';
    $('uninstall-foot').textContent = t('uninstall.openDirFailed', { detail });
    setStatus(t('uninstall.openDirFailed', { detail }), 'error', true);
  } finally {
    button.disabled = false;
  }
}

// ── 滚动条：自绘滑块 ──
// 原生滚动条在这个 WebView 里既不能只画滑块（标准属性会带出箭头和轨道），
// ::-webkit-scrollbar 也不生效，所以整体隐藏，用一根 5px 圆角滑块代替。
// 规则：指针进入容器、或焦点落进容器、或正在滚动时出现；离开后 0.9s 淡出。
const SCROLL_THUMB_CLASSES = ['plugins-panel', 'uninstall-steps'];
const SCROLL_THUMB_WIDTH = 5;
const SCROLL_THUMB_GAP = 4;
const SCROLL_THUMB_MIN = 26;
const SCROLL_THUMB_LINGER = 900;

function initScrollThumb() {
  const thumb = document.createElement('div');
  thumb.className = 'scroll-thumb';
  document.body.appendChild(thumb);

  let active = null;
  let hovering = false;
  let hideTimer = 0;

  const hide = () => {
    clearTimeout(hideTimer);
    thumb.dataset.on = 'false';
    active = null;
  };

  const linger = () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (!hovering) thumb.dataset.on = 'false';
    }, SCROLL_THUMB_LINGER);
  };

  // 启动页只有在窄窗口下才会真的滚动（body{overflow:auto} 的媒体查询）。
  // 找到真正在滚的那个元素：body 或 html。
  const pageScroller = () => {
    const body = document.body;
    const root = document.documentElement;
    if (body.scrollHeight - body.clientHeight > 1) return body;
    if (root.scrollHeight - root.clientHeight > 1) return root;
    return null;
  };

  const pageScrollable = () => {
    const overflow = getComputedStyle(document.body).overflowY;
    if (overflow !== 'auto' && overflow !== 'scroll') return false;
    return Boolean(pageScroller());
  };

  const surfaceOf = (node) => {
    let el = node instanceof Element ? node : null;
    while (el && el !== document.documentElement) {
      if (el === document.body) return pageScrollable() ? document.body : null;
      if (SCROLL_THUMB_CLASSES.some((name) => el.classList.contains(name))) return el;
      el = el.parentElement;
    }
    return null;
  };

  const viewOf = (el) => {
    if (el === document.body) {
      const scroller = pageScroller() || document.scrollingElement || document.documentElement;
      return {
        top: 0,
        height: scroller.clientHeight,
        x: window.innerWidth - SCROLL_THUMB_WIDTH - SCROLL_THUMB_GAP,
        current: scroller.scrollTop,
        total: scroller.scrollHeight,
        size: scroller.clientHeight,
      };
    }
    const rect = el.getBoundingClientRect();
    return {
      top: rect.top + el.clientTop,
      height: el.clientHeight,
      x:
        rect.left +
        el.clientLeft +
        el.clientWidth -
        SCROLL_THUMB_WIDTH -
        SCROLL_THUMB_GAP,
      current: el.scrollTop,
      total: el.scrollHeight,
      size: el.clientHeight,
    };
  };

  const paint = () => {
    if (!active || !active.isConnected) return hide();
    const view = viewOf(active);
    const range = view.total - view.size;
    if (range <= 1 || view.height <= 0) return hide();
    const height = Math.max(
      SCROLL_THUMB_MIN,
      Math.round(view.height * (view.size / view.total)),
    );
    const progress = Math.min(1, Math.max(0, view.current / range));
    thumb.style.left = `${Math.round(view.x)}px`;
    thumb.style.top = `${Math.round(view.top + progress * (view.height - height))}px`;
    thumb.style.height = `${height}px`;
    thumb.dataset.on = 'true';
  };

  document.addEventListener(
    'pointerover',
    (event) => {
      const el = surfaceOf(event.target);
      if (!el) return;
      active = el;
      hovering = true;
      clearTimeout(hideTimer);
      paint();
    },
    true,
  );

  document.addEventListener(
    'pointerout',
    (event) => {
      const el = surfaceOf(event.target);
      if (!el) return;
      const next = event.relatedTarget;
      if (next instanceof Node && el.contains(next)) return;
      hovering = false;
      if (el.contains(document.activeElement)) linger();
      else hide();
    },
    true,
  );

  const onScroll = (event) => {
    const node = event.target;
    let el = null;
    if (node === document || node === document.documentElement) el = document.body;
    else if (node instanceof Element) el = surfaceOf(node);
    if (!el) return;
    active = el;
    paint();
    if (!hovering) linger();
  };
  document.addEventListener('scroll', onScroll, true);
  window.addEventListener('scroll', onScroll, true);

  document.addEventListener(
    'focusin',
    (event) => {
      const el = surfaceOf(event.target);
      if (!el) return;
      active = el;
      paint();
    },
    true,
  );

  document.addEventListener(
    'focusout',
    () => {
      if (!hovering && active) linger();
    },
    true,
  );

  window.addEventListener('resize', () => {
    if (active) paint();
  });
}

/** 椰果娘随机眨眼：每隔 2–6 秒闭眼约 110ms。 */
function startBlinking() {
  const blink = $('coconut-blink');
  if (!blink) return;
  function once() {
    blink.classList.add('on');
    setTimeout(() => blink.classList.remove('on'), 110);
    setTimeout(once, 2200 + Math.random() * 3600);
  }
  setTimeout(once, 1800);
}

/** 背景蓝图装饰线：用可复现的伪随机点算法生成三角尺刻度 / 圆弧 / 辅助线。 */
function drawBlueprint() {
  const svg = $('blueprint');
  if (!svg) return;
  const W = 1180, H = 720;
  let s = '';
  const C = 'var(--blue)', CS = 'var(--blue-strong)';
  const line = (x1, y1, x2, y2, c, w, op, dash) =>
    `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${c || C}" stroke-width="${w || 1}" opacity="${op || 0.5}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`;
  const circ = (cx, cy, r, c, w, op, dash) =>
    `<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${c || C}" stroke-width="${w || 1}" opacity="${op || 0.5}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`;
  const txt = (x, y, t, op, size) =>
    `<text x="${x}" y="${y}" fill="${CS}" opacity="${op || 0.5}" font-size="${size || 9}" font-family="monospace">${t}</text>`;

  // 左上：圆规弧 + 放射角度刻度
  const ox = 150, oy = 110;
  [80, 130, 180].forEach((r, i) => { s += circ(ox, oy, r, C, 1, 0.26 - i * 0.05, '4 6'); });
  s += circ(ox, oy, 130, CS, 1.1, 0.42);
  for (let a = 0; a <= 180; a += 15) {
    const rad = a * Math.PI / 180;
    const r1 = a % 45 === 0 ? 138 : 134;
    const r2 = a % 45 === 0 ? 150 : 140;
    s += line(ox + r1 * Math.cos(rad), oy - r1 * Math.sin(rad), ox + r2 * Math.cos(rad), oy - r2 * Math.sin(rad), CS, 1, a % 45 === 0 ? 0.48 : 0.3);
    if (a % 45 === 0) s += txt(ox + 162 * Math.cos(rad) - 8, oy - 162 * Math.sin(rad) + 3, a + '°', 0.34);
  }
  s += line(ox - 185, oy, ox + 185, oy, C, 1, 0.2, '2 6');
  s += line(ox, oy - 185, ox, oy + 185, C, 1, 0.2, '2 6');
  s += circ(ox, oy, 3, CS, 1.4, 0.5);

  // 右下：直角三角尺 + 刻度
  const tx = 830, ty = 470, base = 280, hgt = 165;
  s += `<polygon points="${tx},${ty} ${tx + base},${ty} ${tx},${ty - hgt}" fill="none" stroke="${CS}" stroke-width="1.3" opacity="0.46"/>`;
  const m = 28;
  s += `<polygon points="${tx + m},${ty - m} ${tx + base - m - 45},${ty - m} ${tx + m},${ty - m - hgt + m + 32}" fill="none" stroke="${C}" stroke-width="1" opacity="0.34"/>`;
  for (let i = 1; i < base / 12; i++) {
    const x = tx + i * 12, long = i % 5 === 0;
    s += line(x, ty, x, ty - (long ? 9 : 5), CS, 1, long ? 0.48 : 0.32);
    if (long) s += txt(x - 6, ty + 15, String(i * 12), 0.38, 8);
  }
  for (let i = 1; i < hgt / 12; i++) {
    const y = ty - i * 12, long = i % 5 === 0;
    s += line(tx, y, tx + (long ? 9 : 5), y, CS, 1, long ? 0.48 : 0.32);
  }
  s += `<rect x="${tx}" y="${ty - 12}" width="12" height="12" fill="none" stroke="${CS}" stroke-width="1" opacity="0.46"/>`;

  // 中部：贯穿辅助线 + 端点
  s += line(380, 80, 820, 560, C, 1, 0.18, '6 7');
  s += line(340, 560, 760, 210, C, 1, 0.14, '6 7');
  [[380, 80], [820, 560], [340, 560], [760, 210]].forEach((p) => {
    s += circ(p[0], p[1], 4, C, 1, 0.38);
    s += circ(p[0], p[1], 1.5, CS, 1, 0.5);
  });

  // 散布装饰点（可复现）
  let seed = 20251119;
  const rnd = () => { seed = (seed * 9301 + 49297) % 233280; return seed / 233280; };

  // ── 追加的几何装饰（都是压在文字与立绘下面的背景层） ──
  const arc = (cx, cy, r, a0, a1, c, w, op, dash) => {
    const p0 = [cx + r * Math.cos((a0 * Math.PI) / 180), cy - r * Math.sin((a0 * Math.PI) / 180)];
    const p1 = [cx + r * Math.cos((a1 * Math.PI) / 180), cy - r * Math.sin((a1 * Math.PI) / 180)];
    const large = Math.abs(a1 - a0) > 180 ? 1 : 0;
    return `<path d="M${p0[0].toFixed(1)} ${p0[1].toFixed(1)} A${r} ${r} 0 ${large} 0 ${p1[0].toFixed(1)} ${p1[1].toFixed(1)}" fill="none" stroke="${c || C}" stroke-width="${w || 1}" opacity="${op || 0.4}"${dash ? ` stroke-dasharray="${dash}"` : ''}/>`;
  };
  const cross = (x, y, size, op) =>
    line(x - size, y, x + size, y, CS, 1, op || 0.4) + line(x, y - size, x, y + size, CS, 1, op || 0.4);

  // 左上角：细网格（工程图纸的底纹）
  for (let i = 0; i <= 6; i++) {
    s += line(28 + i * 26, 24, 28 + i * 26, 24 + 156, C, 1, i % 3 === 0 ? 0.14 : 0.07);
    s += line(28, 24 + i * 26, 28 + 156, 24 + i * 26, C, 1, i % 3 === 0 ? 0.14 : 0.07);
  }
  s += txt(30, 196, 'GRID 26', 0.26, 8);

  // 左下角：同心环 + 放射刻度 + 半径标注
  const lx = 120, ly = 640;
  [40, 78, 118, 165].forEach((r, i) => { s += circ(lx, ly, r, i === 2 ? CS : C, 1, 0.3 - i * 0.045, i % 2 ? '3 5' : ''); });
  for (let a = 0; a < 360; a += 10) {
    const rad = (a * Math.PI) / 180;
    const outer = a % 30 === 0 ? 172 : 168;
    s += line(
      lx + 165 * Math.cos(rad), ly - 165 * Math.sin(rad),
      lx + outer * Math.cos(rad), ly - outer * Math.sin(rad),
      CS, 1, a % 30 === 0 ? 0.34 : 0.18,
    );
  }
  s += line(lx, ly, lx + 165 * Math.cos(Math.PI / 6), ly - 165 * Math.sin(Math.PI / 6), CS, 1, 0.36, '5 4');
  s += txt(lx + 74, ly - 58, 'R 165', 0.32, 8);
  s += cross(lx, ly, 6, 0.46);

  // 右上角：坐标轴 + 抛物线 + 采样点
  const rx = 1044, ry = 150;
  s += line(rx - 190, ry, rx + 34, ry, C, 1, 0.24);
  s += line(rx, ry + 150, rx, ry - 74, C, 1, 0.24);
  s += `<path d="M${rx - 168} ${ry + 96} Q${rx - 74} ${ry - 96} ${rx + 18} ${ry + 34}" fill="none" stroke="${CS}" stroke-width="1.2" opacity="0.4" stroke-dasharray="7 5"/>`;
  for (let i = 0; i < 6; i++) {
    const t = i / 5;
    const px = rx - 168 + 186 * t;
    const py = ry + 96 - 192 * t + 130 * t * t;
    s += circ(px, py, 2.4, CS, 1, 0.42);
  }
  s += txt(rx + 40, ry - 66, 'f(x)', 0.3, 8);
  s += txt(rx - 186, ry + 16, 'O', 0.34, 9);

  // 中下：半圆量角器（180° 刻度）
  const gx = 560, gy = 700;
  [86, 118].forEach((r, i) => { s += arc(gx, gy, r, 0, 180, i ? CS : C, 1, 0.28, i ? '' : '4 6'); });
  for (let a = 0; a <= 180; a += 5) {
    const rad = (a * Math.PI) / 180;
    const long = a % 30 === 0;
    const r1 = 118;
    const r2 = long ? 104 : 111;
    s += line(gx + r1 * Math.cos(rad), gy - r1 * Math.sin(rad), gx + r2 * Math.cos(rad), gy - r2 * Math.sin(rad), CS, 1, long ? 0.34 : 0.18);
  }
  s += line(gx - 132, gy, gx + 132, gy, C, 1, 0.2, '2 6');

  // 右下：斜向排线阴影（等距平行线）
  for (let i = 0; i < 14; i++) {
    const off = i * 15;
    s += line(950 + off, 720, 1180 + off, 490, C, 1, 0.06);
  }

  // 顶部横贯尺寸线（带端点与数值）
  s += line(260, 54, 900, 54, CS, 1, 0.24, '5 5');
  s += line(260, 44, 260, 64, CS, 1, 0.32);
  s += line(900, 44, 900, 64, CS, 1, 0.32);
  s += txt(548, 48, '960', 0.3, 8);

  // 几个定位十字
  [[420, 300], [980, 620], [700, 120], [120, 300]].forEach(([x, y]) => { s += cross(x, y, 7, 0.3); });

  // 小三角 / 菱形点缀
  s += `<polygon points="880,300 918,300 899,266" fill="none" stroke="${CS}" stroke-width="1" opacity="0.28"/>`;
  s += `<polygon points="240,430 268,444 240,458 212,444" fill="none" stroke="${C}" stroke-width="1" opacity="0.26"/>`;

  for (let i = 0; i < 64; i++) {
    const x = 30 + rnd() * (W - 60), y = 30 + rnd() * (H - 60), r = rnd() * 1.6 + 0.5;
    s += `<circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${r.toFixed(1)}" fill="${CS}" opacity="${(0.1 + rnd() * 0.2).toFixed(2)}"/>`;
  }
  svg.innerHTML = s;
}

/**
 * 点阵聚光灯：把鼠标位置写进 CSS 变量 --cursor-x / --cursor-y，
 * `.app::after` 用一层跟着光标走的径向蒙版，把光标附近的点阵提亮。
 * 节流到每帧一次，避免 pointermove 高频写样式。
 */
function initCursorDots() {
  const app = document.querySelector('.app');
  if (!app) return;
  let frame = 0;
  let x = -400;
  let y = -400;

  const paint = () => {
    frame = 0;
    app.style.setProperty('--cursor-x', `${x}px`);
    app.style.setProperty('--cursor-y', `${y}px`);
  };

  const onMove = (event) => {
    const rect = app.getBoundingClientRect();
    x = event.clientX - rect.left;
    y = event.clientY - rect.top;
    app.classList.add('cursor-on');
    if (!frame) frame = requestAnimationFrame(paint);
  };

  // pointermove 和 mousemove 都挂上：老 WebView2 上个别情况只来其中一个
  window.addEventListener('pointermove', onMove, { passive: true });
  window.addEventListener('mousemove', onMove, { passive: true });
  window.addEventListener('pointerdown', onMove, { passive: true });

  // 指针离开窗口就把聚光灯淡掉
  const dim = () => app.classList.remove('cursor-on');
  window.addEventListener('blur', dim);
  document.addEventListener('pointerleave', dim);
  app.addEventListener('pointerleave', dim);
}

// 进入启动页时自动检查一次更新：工具箱与游戏数据串行执行，避免 busy 锁冲突。// 只做「检查」，不自动下载更新——发现新版本仍由用户点按钮触发更新。
async function autoCheckOnEnter() {
  if (!connected || busy) return;
  autoSeriesStopped = false;
  await runCheck('toolbox');
  // 用户点了「停止更新」就把后面那次游戏数据检查也一起停掉。
  if (autoSeriesStopped) return;
  // runCheck 结束会释放 busy；等一拍再查游戏数据，避免同一 tick 抢占。
  await new Promise((resolve) => setTimeout(resolve, 350));
  if (autoSeriesStopped || !connected || busy) return;
  await runCheck('game');
}

async function init() {
  // 首屏先用系统主题铺一下，但**先不落盘**：不然每次启动都会把用户选的主题覆盖掉。
  paintTheme(darkQuery.matches ? 'dark' : 'light');
  invoke('set_window_theme', { theme: currentTheme }).catch(() => {});
  const onSystemTheme = (event) => {
    if (themeLocked) return;
    applyTheme(event.matches ? 'dark' : 'light');
  };
  if (darkQuery.addEventListener) darkQuery.addEventListener('change', onSystemTheme);
  else if (darkQuery.addListener) darkQuery.addListener(onSystemTheme);

  $('start').addEventListener('click', onStartClick);
  $('check-toolbox').addEventListener('click', () => runCheck('toolbox'));
  $('check-game').addEventListener('click', () => runCheck('game'));
  // 启动页语言跟随系统；顺手告诉 Rust，进度文案用同一种语言。
  invoke('set_language', { lang: window.COCONUT_LANG.locale }).catch(() => {});
  $('uninstall').addEventListener('click', runUninstall);
  initPlugins();
  $('uninstall-exit').addEventListener('click', handleUninstallPrimary);
  $('uninstall-exit-app').addEventListener('click', () => exitLauncher($('uninstall-exit-app')));
  $('theme-toggle').addEventListener('click', toggleTheme);
  $('win-min').addEventListener('click', () => {
    invoke('window_minimize').catch(() => {});
  });
  $('win-max').addEventListener('click', () => {
    invoke('window_toggle_maximize').catch(() => {});
  });
  $('win-close').addEventListener('click', () => {
    invoke('window_close').catch(() => {});
  });

  await refresh();
  // 存过主题就听它的（get_state.theme）并锁定；没存过就把系统主题写进去。
  if (state && (state.theme === 'dark' || state.theme === 'light')) {
    themeLocked = true;
    paintTheme(state.theme);
    invoke('set_window_theme', { theme: currentTheme }).catch(() => {});
  } else if (connected) {
    themeLocked = true;
    applyTheme(currentTheme);
  }
  loadPlugins();
  initScrollThumb();
  fitTitle();
  fitByline();
  // 每次进入自动检查一次更新（工具箱 + 游戏数据，串行）。
  setTimeout(() => autoCheckOnEnter(), 600);
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready
      .then(() => {
        fitTitle();
        fitByline();
      })
      .catch(() => {});
  }
  let bylineTimer = 0;
  window.addEventListener('resize', () => {
    clearTimeout(bylineTimer);
    bylineTimer = setTimeout(() => {
      fitTitle();
      fitByline();
    }, 120);
  });
  // 点椰果图标切语言：按 zh-Hans → zh-Hant → ja → ko → en → zh-Hans 循环，不弹窗
  const cycleLanguage = () => {
    const api = window.COCONUT_LANG;
    if (!api || !api.setLocale) return;
    const order = api.supported || ['zh-Hans', 'zh-Hant', 'ja', 'ko', 'en'];
    const index = order.indexOf(api.locale);
    const next = order[(index + 1) % order.length];
    api.setLocale(next);
    invoke('set_language', { lang: next }).catch(() => {});
    // 文案长度变了，标题 / 副标题要重新量一次
    fitTitle();
    fitByline();
  };
  $('coconut').addEventListener('click', cycleLanguage);
  // 椰果娘眨眼
  startBlinking();
  // 蓝图装饰线
  drawBlueprint();
  // 点阵聚光灯（跟随鼠标）
  initCursorDots();

  const poll = async () => {
    await refresh();
    // 有任务在跑时加快轮询：停止按钮 / 自动更新的状态要尽快反映到界面上。
    setTimeout(poll, busy || backendActivity ? 600 : 1500);
  };
  setTimeout(poll, 1500);
}

window.addEventListener('DOMContentLoaded', init);