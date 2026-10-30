/**
 * 页面侧代码：普通函数，跟着插件一起被 esbuild 压缩，
 * 注入时用 `Function.prototype.toString()` 序列化，不再手写模板字符串。
 *
 * 约束：函数必须**完全自包含**，不能引用模块作用域里的任何东西。
 *
 * 工具箱的 run_js 会把代码塞进 `return (async () => { ... })()`，
 * 所以这里返回 Promise 也没问题，宿主会等它。
 */

/** 读工具箱 ai-gen-prefs：{ found, enabled } / { error: true }。 */
export async function readAiPrefs() {
  const value = await new Promise((resolve) => {
    const req = indexedDB.open('wuwa-v1', 1);
    req.onerror = () => resolve({ error: true });
    req.onsuccess = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('cache')) {
        resolve({ found: false });
        return;
      }
      const get = db.transaction('cache', 'readonly').objectStore('cache').get('ai-gen-prefs');
      get.onerror = () => resolve({ error: true });
      get.onsuccess = () => {
        const record = get.result;
        const data = record && record.data ? record.data : null;
        if (!data) {
          resolve({ found: false });
          return;
        }
        resolve({ found: true, enabled: data.enabled === true });
      };
    };
  });
  return JSON.stringify(value);
}

/** 读工具箱 ai-config 里已经落盘的配置条数：{ profiles: [...] } / null。 */
export async function readAiConfig() {
  const value = await new Promise((resolve) => {
    const req = indexedDB.open('wuwa-v1', 1);
    req.onerror = () => resolve(null);
    req.onsuccess = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('cache')) {
        resolve(null);
        return;
      }
      const get = db.transaction('cache', 'readonly').objectStore('cache').get('ai-config');
      get.onerror = () => resolve(null);
      get.onsuccess = () => {
        const record = get.result;
        resolve(record && record.data ? { profiles: record.data.profiles || [] } : null);
      };
    };
  });
  return JSON.stringify(value);
}

/** 打开工具箱的 AI 悬浮窗：'already' / 'missing' / 'opened'。 */
export function openAssistant() {
  const root = document.querySelector('.ai-assistant');
  if (root && root.getBoundingClientRect().width > 100) return 'already';
  const btn = document.querySelector('.ai-assistant [role="button"][aria-label^="AI 助手"]');
  if (!btn) return 'missing';
  btn.click();
  return 'opened';
}
