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

function disableActions(disabled) {
  // 游戏数据在后台拉取时，只要本地有工具箱静态产物，就允许先启动工具箱。
  // 工具箱静态包自动更新期间也允许启动：更新完会自动刷新页面，不挡用户。
  const canStart = connected && hasStatic && (!disabled || refreshing);
  $('start').disabled = !canStart;
  $('check-toolbox').disabled = disabled || !connected;
  $('check-game').disabled = disabled || !connected;
  $('uninstall').disabled = disabled;
  $('plugins').disabled = !connected;
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
  const off = list.length > 0 && !list.some((plugin) => plugin.enabled);
  button.dataset.error = errored ? 'true' : 'false';
  button.dataset.off = off ? 'true' : 'false';
  const enabled = list.filter((plugin) => plugin.enabled).length;
  $('plugins-label').textContent = off ? t('plugins.allOff') : t('plugins.button');
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

  const enableAll = $('plugins-enable-all');
  const disableAll = $('plugins-disable-all');
  if (enableAll) enableAll.disabled = pluginBusy || total === 0 || enabled === total;
  if (disableAll) disableAll.disabled = pluginBusy || enabled === 0;
  const cleanup = $('plugins-cleanup');
  if (cleanup) cleanup.disabled = pluginBusy;
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
async function cleanupDeadPlugins() {
  if (pluginBusy) return;
  pluginBusy = true;
  updatePluginChrome(pluginEntries);
  try {
    const result = await invoke('plugins_cleanup_dead');
    const states = Number(result?.states || 0);
    const storage = Number(result?.storage || 0);
    if (!states && !storage) {
      setStatus(t('plugins.cleanupEmpty'), 'ready', true);
    } else {
      setStatus(t('plugins.cleanupToast', { states, storage }), 'ready', true);
    }
  } catch (error) {
    setStatus(t('plugins.cleanupFailed', { detail: error?.message || error }), 'error', true);
  }
  pluginBusy = false;
  await loadPlugins();
}

function closePlugins() {
  $('plugins-dialog').hidden = true;
}

async function openPlugins() {
  if (!connected) return;
  $('plugins-dialog').hidden = false;
  await loadPlugins();
  const search = $('plugins-search');
  if (search) {
    search.value = pluginQuery;
    search.focus();
  }
}

function initPlugins() {
  $('plugins').addEventListener('click', openPlugins);
  $('plugins-close').addEventListener('click', closePlugins);
  $('plugins-dialog').addEventListener('click', (event) => {
    if (event.target === $('plugins-dialog')) closePlugins();
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !$('plugins-dialog').hidden) closePlugins();
  });

  $('plugins-open').addEventListener('click', async () => {
    try {
      const dir = await invoke('plugins_open_dir');
      setStatus(t('plugins.dirToast', { dir }), 'ready', true);
    } catch (error) {
      setStatus(t('plugins.openFailed', { detail: error?.message || error }), 'error', true);
    }
  });
  $('plugins-rescan').addEventListener('click', loadPlugins);
  $('plugins-cleanup').addEventListener('click', cleanupDeadPlugins);

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

  $('plugins-enable-all').addEventListener('click', () => bulkPluginAction(true));
  $('plugins-disable-all').addEventListener('click', () => bulkPluginAction(false));
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

  if (!busy && Date.now() >= statusHoldUntil && !next.updating) {
    setPhase('READY');
    if (hasStatic) {
      setStatus(t('status.ready'), 'ready');
    } else {
      setStatus(t('status.missingStatic'), 'error');
    }
  }
  if (next.updating && !busy) {
    setPhase('UPDATE');
    setStatus(t('status.autoUpdating'), 'busy');
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

  busy = true;
  disableActions(true);
  const isToolbox = kind === 'toolbox';
  const button = isToolbox ? 'check-toolbox' : 'check-game';
  const label = isToolbox ? t('check.toolbox') : t('check.game');
  statusHoldUntil = 0;
  // 检查版本也要能换镜像：清单同样是 直连 / 镜像 轮换拉取的。
  updatingToolbox = isToolbox;
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
    const detail = error?.message || String(error) || '';
    setButtonLabel(button, t('check.failed'));
    setStatus(detail || t('check.failed'), 'error', true);
    setProgress(t('progress.checkFailed', { detail }), 'error');
    setPhase('READY');
    scheduleReset(button, label);
  } finally {
    updatingToolbox = false;
    busy = false;
    disableActions(false);
  }
}

async function runUpdate(kind) {
  if (busy || !connected) return;
  const isToolbox = kind === 'toolbox';
  // 拉取游戏数据是后台任务：允许同时启动工具箱。
  refreshing = !isToolbox;
  busy = true;
  disableActions(true);
  const button = isToolbox ? 'check-toolbox' : 'check-game';
  const label = isToolbox ? t('check.toolbox') : t('check.game');
  statusHoldUntil = 0;
  updatingToolbox = isToolbox;
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
    const detail = error?.message || String(error) || '';
    setButtonLabel(button, t('check.failed'));
    setStatus(detail || t('check.failed'), 'error', true);
    setProgress(t('progress.updateFailed', { detail }), 'error');
    setPhase('READY');
    scheduleReset(button, label);
  } finally {
    updatingToolbox = false;
    refreshing = false;
    busy = false;
    disableActions(false);
  }
}

async function startFlow() {
  if (!hasStatic) {
    setStatus(t('status.missingStaticStart'), 'error', true);
    return;
  }
  // 游戏数据在后台拉取时也可以进工具箱。
  if ((busy && !refreshing) || !connected || !state?.url) return;
  busy = true;
  disableActions(true);
  $('start-label').textContent = t('start.busy');
  setPhase('START');
  setStatus(t('status.entering'), 'busy');
  setProgress(t('progress.openingToolbox'), 'busy');
  try {
    await invoke('open_toolbox_window', { url: state.url });
  } catch (error) {
    busy = false;
    disableActions(false);
    $('start-label').textContent = t('start');
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

  $('start').addEventListener('click', startFlow);
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

  const poll = async () => {
    await refresh();
    setTimeout(poll, busy ? 600 : 1500);
  };
  setTimeout(poll, 1500);
}

window.addEventListener('DOMContentLoaded', init);