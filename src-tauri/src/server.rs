use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path as AxumPath, RawQuery, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::Manager;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::i18n::{tr, Lang};
use crate::plugin_host::Plugins;
use crate::plugin_rt::PluginBridge;

pub const TOOLBOX_PORT: u16 = 47821;
pub const WS_PORT: u16 = 8765;
/// 工坊同站反代端口：工坊官方域在 iframe 里是跨站，Cookie 会因 SameSite 丢失；
/// 换成 127.0.0.1 的另一个端口就变成同站（SameSite 只看站点，不看端口）。
pub const WORKSHOP_PORT: u16 = 47822;
const UPSTREAM: &str = "https://opencode.ai/inference/v1";
const API_UPSTREAM: &str = "https://wuwa-afyg-tool.200503.xyz";
const DIST_MANIFEST_URL: &str =
    "https://github.com/d4rkOfficial/wuwa-afyg-tool/releases/latest/download/manifest.json";
const DIST_ARCHIVE_URL: &str =
    "https://github.com/d4rkOfficial/wuwa-afyg-tool/releases/latest/download/toolbox.tar.gz";
// GitHub Release 资产走 release-assets.githubusercontent.com，部分网络会重置连接。
// 直连失败后依次尝试这些 GitHub 加速镜像；镜像只是兜底，不参与 manifest 检查。
const DIST_ARCHIVE_MIRRORS: &[&str] = &["https://gh-proxy.com/", "https://ghfast.top/"];
const USER_AGENT: &str = "opencode/1.19.3";
// 免费模型列表一天只向上游拉一次，避免频繁握手触发风控。
const MODELS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
// 列表抓取失败（冷启动无缓存）时的重试间隔。
const MODELS_RETRY: Duration = Duration::from_secs(10 * 60);
/// 下载 / 读清单时「多久收不到数据」就放弃当前来源，转下一个镜像。
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const PUBLIC_KEY: &str = "public";
const MIRROR_SWITCHED: &str = "__mirror_switched__";
/// 工具箱自带 PWA Service Worker，会预缓存 `/` 和 `_app` 资源。
/// 启动器每次进工具箱都换一份「空壳 + 清预缓存」的 sw.js：
/// 旧版本被缓存住、必须 Ctrl+Shift+R 才能看到新版本，就是它干的。
const SW_KILL: &str = r#"/* 启动器接管：静态文件由本地服务直接提供，工具箱不需要 Service Worker 离线缓存。
   这里把旧版本 Workbox 预缓存全部清掉（保留业务自己的图片缓存），并让 SW 变成不拦截任何请求的空壳。 */
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    let cleared = false;
    try {
      const names = await caches.keys();
      for (const name of names) {
        if (!/workbox|precache/i.test(name)) continue;
        if (await caches.delete(name)) cleared = true;
      }
    } catch (error) {}
    try {
      await self.clients.claim();
    } catch (error) {}
    if (!cleared) return;
    try {
      const clients = await self.clients.matchAll({ type: 'window' });
      for (const client of clients) client.navigate(client.url).catch(() => {});
    } catch (error) {}
  })());
});
"#;
// 注入工具箱的沉浸式外壳脚本（run_js）。
const SHELL_INJECT: &str = include_str!("shell_inject.js");
/// 沉浸式标题栏的缩放范围（Ctrl + - / = 调整，步进 5%）。
const ZOOM_MIN: f64 = 0.9;
const ZOOM_MAX: f64 = 1.2;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct GameData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub fetched_at: String,
    #[serde(default)]
    pub entries: HashMap<String, Value>,
}

pub struct ServerState {
    pub static_dir: PathBuf,
    pub data_dir: PathBuf,
    pub game_file: PathBuf,
    /// 本地化的图标（解压后的 webp）
    pub icons_dir: PathBuf,
    pub game: RwLock<GameData>,
    pub client: reqwest::Client,
    pub models: RwLock<Vec<String>>,
    pub models_at: RwLock<Option<Instant>>,
    pub models_file: PathBuf,
    pub api_key: RwLock<String>,
    pub key_file: PathBuf,
    pub mirror_index: AtomicUsize,
    pub mirror_gen: AtomicUsize,
    pub theme: RwLock<String>,
    pub commit: RwLock<String>,
    pub build: RwLock<String>,
    pub progress: RwLock<String>,
    pub url: String,
    pub ready: AtomicBool,
    pub error: RwLock<String>,
    pub ws: WsHub,
    pub app: RwLock<Option<tauri::AppHandle>>,
    /// 工具箱整体缩放，只支持 100% / 120% 两档；用 f64 的位表示存进原子量。
    pub zoom: AtomicU64,
    /// 游戏数据是否正在后台拉取。
    pub crawling: AtomicBool,
    /// 工坊反代的当前上游（由 iframe 的 `?__coconut_target=` 设置，绝对路径请求沿用它）。
    pub workshop_target: RwLock<Option<String>>,
    /// 工具箱静态包是否正在下载 / 替换（自动更新与手动更新互斥）。
    pub updating: AtomicBool,
    /// 插件管理器（内置 + 用户插件）。
    pub plugins: Plugins,
    /// 启动页语言（前端按系统语言探测后通过 set_language 告诉这边）
    pub lang: RwLock<Lang>,
    /// WS 帧广播：debug 插件订阅用。
    pub frames: tokio::sync::broadcast::Sender<Value>,
    /// 最后一个「有 run_js 的」页面会话：插件的宿主调用优先用它，
    /// 免去「刚建立 / 正要关闭」那一瞬间在 hub 里找不到会话的尴尬。
    pub page_session: RwLock<Option<Arc<WsSession>>>,
    /// 上面这个开关的落盘位置（data_dir/settings.json）。
    pub settings_file: PathBuf,
}

pub struct WsHub {
    sessions: Mutex<Vec<Arc<WsSession>>>,
}

pub struct WsSession {
    id: String,
    tx: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    tools: Mutex<HashSet<String>>,
    closed: AtomicBool,
    /// 往帧广播里塞一条记录（debug 插件订阅用）
    frames: tokio::sync::broadcast::Sender<Value>,
}

impl WsSession {
    async fn has_tool_async(&self, name: &str) -> bool {
        self.tools.lock().await.contains(name)
    }

    async fn exec(&self, tool: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("WS 连接已关闭".into());
        }
        let id = format!(
            "{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            rand::thread_rng().gen::<u32>()
        );
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        let message = json!({
            "type": "exec",
            "id": id,
            "tool": tool,
            "args": args,
        });
        let _ = self.frames.send(json!({
            "direction": "send",
            "frame": message,
            "at": now_millis(),
        }));
        if self.tx.send(message.to_string()).is_err() {
            return Err("WS 发送失败".into());
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(value)) => {
                if value.get("ok").and_then(Value::as_bool) == Some(false) {
                    Err(value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("工具执行失败")
                        .to_string())
                } else {
                    // 工具箱的 result 是 {type,id,ok,data}，data 才是工具返回值：
                    // run_js -> {value,logs}，其它工具 -> 工具自己的对象。
                    Ok(value
                        .get("data")
                        .cloned()
                        .unwrap_or_else(|| {
                            let mut payload = value;
                            if let Some(object) = payload.as_object_mut() {
                                object.remove("type");
                                object.remove("id");
                                object.remove("ok");
                            }
                            payload
                        }))
                }
            }
            Ok(Err(_)) => Err("WS 请求被取消".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("工具调用超时：{tool}"))
            }
        }
    }
}

impl WsHub {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(Vec::new()),
        }
    }

    async fn add(&self, session: Arc<WsSession>) {
        self.sessions.lock().await.push(session);
    }

    async fn remove(&self, id: &str) {
        self.sessions.lock().await.retain(|s| s.id != id);
    }

    async fn snapshot(&self) -> Vec<Arc<WsSession>> {
        self.sessions.lock().await.clone()
    }

    /// 让插件重新跑一次同步（免费模型插件监听 resync 事件）。
    pub async fn sync_all(&self, state: Arc<ServerState>) {
        state.plugins.event(json!({ "type": "resync" }));
    }

    pub async fn set_theme(&self, theme: &str) {
        for session in self.snapshot().await {
            if session.has_tool_async("set_setting").await {
                let _ = session
                    .exec(
                        "set_setting",
                        json!({ "key": "theme_mode", "value": theme }),
                        Duration::from_secs(10),
                    )
                    .await;
            }
        }
    }
}

impl ServerState {
    pub async fn set_progress(&self, text: impl Into<String>) {
        *self.progress.write().await = text.into();
    }

    pub fn log(&self, tag: &str, message: &str) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("[{stamp}] [{tag}] {message}\n");
        let path = self.data_dir.join("service.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub async fn model_ids(&self, force: bool) -> Vec<String> {
        let fresh = {
            let at = *self.models_at.read().await;
            at.map(|value| value.elapsed() < MODELS_TTL)
                .unwrap_or(false)
        };
        if !force && fresh {
            return self.models.read().await.clone();
        }

        // 磁盘缓存仍然新鲜：一天内重启启动器也不重新打上游。
        if let Some((cached, stamp)) = read_models_cache(&self.models_file) {
            let age = SystemTime::now()
                .duration_since(stamp)
                .unwrap_or_default();
            if age < MODELS_TTL {
                let mut models = self.models.write().await;
                if *models != cached {
                    *models = cached.clone();
                }
                drop(models);
                if let Some(at) = Instant::now().checked_sub(age) {
                    *self.models_at.write().await = Some(at);
                }
                return cached;
            }
        }

        let response = self
            .client
            .get(format!("{UPSTREAM}/models"))
            .header("user-agent", USER_AGENT)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(8))
            .send()
            .await;

        if let Ok(response) = response {
            if response.status().is_success() {
                if let Ok(payload) = response.json::<Value>().await {
                    let ids = extract_public_models(&payload);
                    if !ids.is_empty() {
                        *self.models.write().await = ids.clone();
                        *self.models_at.write().await = Some(Instant::now());
                        write_models_cache(&self.models_file, &ids);
                        self.log("models", &format!("上游免费模型已更新：{}", ids.join(", ")));
                        return ids;
                    }
                }
            }
        }

        self.log("models", "拉取上游模型列表失败，沿用上次结果");
        let current = self.models.read().await.clone();
        if !current.is_empty() {
            return current;
        }
        // 内存为空时退回磁盘上的旧缓存（即使已经过期），总比没有模型好。
        if let Some((cached, _)) = read_models_cache(&self.models_file) {
            *self.models.write().await = cached.clone();
            return cached;
        }
        Vec::new()
    }

    /// 当前主题（light / dark）。
    pub async fn theme(&self) -> String {
        self.theme.read().await.clone()
    }

    /// 切换主题并落盘，同时把 theme_mode 推给工具箱。
    pub async fn set_theme(&self, theme: &str) -> Result<String, String> {
        let value = if theme == "dark" { "dark" } else { "light" };
        *self.theme.write().await = value.to_string();
        let language = self.lang.read().await.tag().to_string();
        write_settings(
            &self.settings_file,
            LauncherSettings {
                language,
                theme: value.to_string(),
            },
        )?;
        self.ws.set_theme(value).await;
        Ok(value.to_string())
    }

    /// 当前启动页语言（进度行 / 卸载步骤都用它）。
    pub async fn lang(&self) -> Lang {
        *self.lang.read().await
    }

    /// 改语言并落盘。返回是否发生变化。
    pub async fn set_language(&self, tag: &str) -> Result<bool, String> {
        let next = Lang::from_tag(tag);
        let mut slot = self.lang.write().await;
        let changed = *slot != next;
        *slot = next;
        drop(slot);
        if changed {
            let theme = self.theme.try_read().map(|theme| theme.clone()).unwrap_or_default();
            write_settings(
                &self.settings_file,
                LauncherSettings {
                    language: next.tag().to_string(),
                    theme,
                },
            )?;
        }
        Ok(changed)
    }

    /// 读日志文件的最后 N 行（工具箱里的只读 Console 用）。
    /// source：`plugins`（插件日志）或 `service`（启动器运行日志）。
    pub fn log_tail(&self, source: &str, limit: usize) -> Vec<String> {
        let path = match source {
            "service" => self.data_dir.join("service.log"),
            _ => self.plugins.log_path().to_path_buf(),
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.len() > limit {
            lines = lines.split_off(lines.len() - limit);
        }
        lines
    }

    pub async fn set_api_key(&self, key: String) -> Result<(), String> {
        let key = key.trim().to_string();
        if !key.is_empty() && !looks_like_api_key(&key) {
            return Err("opencode API Key 格式不合法（只允许字母、数字和 - _ . ~ + / =）".into());
        }
        *self.api_key.write().await = key.clone();
        let payload = json!({ "key": key });
        std::fs::write(
            &self.key_file,
            serde_json::to_vec_pretty(&payload).unwrap_or_default(),
        )
        .map_err(|error| format!("写入 opencode Key 失败：{error}"))?;
        self.log(
            "key",
            if key.is_empty() {
                "已清除 opencode API Key"
            } else {
                "已更新 opencode API Key"
            },
        );
        Ok(())
    }

    /// 当前调用方应当转发给上游的 Authorization 值。
    pub async fn client_auth(&self, caller: Option<&str>) -> String {
        // 兼容 `Bearer xxx` / `bearer xxx` / 直接给 key 三种写法。
        let caller_key = caller.map(str::trim).map(strip_bearer).unwrap_or("");
        if looks_like_api_key(caller_key) {
            return format!("Bearer {caller_key}");
        }
        let stored = self.api_key.read().await.clone();
        if looks_like_api_key(&stored) {
            return format!("Bearer {stored}");
        }
        // 格式不合法或未配置：按公开额度处理。
        format!("Bearer {PUBLIC_KEY}")
    }

    pub fn mirror_count(&self) -> usize {
        1 + DIST_ARCHIVE_MIRRORS.len()
    }

    pub fn switch_mirror(&self) -> usize {
        let next = (self.mirror_index.load(Ordering::SeqCst) + 1) % self.mirror_count();
        self.mirror_index.store(next, Ordering::SeqCst);
        self.mirror_gen.fetch_add(1, Ordering::SeqCst);
        self.log(
            "update",
            &format!("用户切换镜像到 {}", mirror_label(Lang::ZhCn, next)),
        );
        next
    }

    pub async fn set_app(&self, app: tauri::AppHandle) {
        *self.app.write().await = Some(app);
    }

    pub fn zoom(&self) -> f64 {
        f64::from_bits(self.zoom.load(Ordering::SeqCst))
    }

    fn store_zoom(&self, value: f64) {
        self.zoom.store(value.to_bits(), Ordering::SeqCst);
    }

    fn main_window(&self) -> Option<tauri::WebviewWindow> {
        // 只在真正处理指令时短暂持锁，避免跨 await 持锁。
        let app = self.app.try_read().ok().and_then(|slot| slot.clone())?;
        app.get_webview_window("main")
    }

    /// 处理注入脚本发来的窗口指令（沉浸式标题栏：缩放 / 最小化 / 缩放 / 关闭 / 拖动）。
    pub async fn window_action(&self, message: &Value) -> Value {
        let action = message["action"].as_str().unwrap_or("");
        let Some(window) = self.main_window() else {
            return json!({ "type": "window-state", "zoom": self.zoom(), "ok": false });
        };
        let mut ok = true;
        match action {
            "state" => {}
            "zoom-set" => {
                // 只认 90% ~ 120%，步进由前端按 5% 计算。
                let requested = message["zoom"].as_f64().unwrap_or_else(|| self.zoom());
                let next = ((requested * 100.0).round() / 100.0).clamp(ZOOM_MIN, ZOOM_MAX);
                if (next - self.zoom()).abs() > f64::EPSILON {
                    self.store_zoom(next);
                    if let Err(error) = window.set_zoom(next) {
                        ok = false;
                        self.log("window", &format!("设置缩放失败：{error}"));
                    }
                }
            }
            "minimize" => {
                if let Err(error) = window.minimize() {
                    ok = false;
                    self.log("window", &format!("最小化失败：{error}"));
                }
            }
            "maximize-toggle" => {
                let result = if window.is_maximized().unwrap_or(false) {
                    window.unmaximize()
                } else {
                    window.maximize()
                };
                if let Err(error) = result {
                    ok = false;
                    self.log("window", &format!("缩放窗口失败：{error}"));
                }
            }
            "close" => {
                if let Err(error) = window.close() {
                    ok = false;
                    self.log("window", &format!("关闭窗口失败：{error}"));
                }
            }
            "drag" => {
                // Windows 上直接走系统拖拽（WM_NCLBUTTONDOWN），比自算位移顺滑。
                if let Err(error) = window.start_dragging() {
                    ok = false;
                    self.log("window", &format!("拖动窗口失败：{error}"));
                }
            }
            _ => ok = false,
        }
        json!({ "type": "window-state", "zoom": self.zoom(), "ok": ok })
    }
}

/// 镜像序号 → 本地化名字（0 = 直连）。
pub fn mirror_label(lang: Lang, index: usize) -> String {
    if index == 0 {
        tr!(lang, "直连", "直連", "直接接続", "직접 연결", "Direct").to_string()
    } else {
        match lang {
            Lang::ZhCn => format!("镜像 {index}"),
            Lang::ZhTw => format!("鏡像 {index}"),
            Lang::Ja => format!("ミラー {index}"),
            Lang::Ko => format!("미러 {index}"),
            Lang::En => format!("Mirror {index}"),
        }
    }
}

/// 直连 + 各镜像的前缀（直连是空前缀）。清单和静态包共用同一套源。
fn mirror_bases() -> Vec<(usize, String)> {
    let mut bases = vec![(0usize, String::new())];
    for (index, mirror) in DIST_ARCHIVE_MIRRORS.iter().enumerate() {
        bases.push((index + 1, mirror.to_string()));
    }
    bases
}

/// 从 `start` 开始轮换：直连 -> 镜像 1 -> 镜像 2 -> 直连…
fn rotated_bases(start: usize) -> Vec<(usize, String)> {
    let bases = mirror_bases();
    let len = bases.len();
    let start = if len == 0 { 0 } else { start % len };
    (0..len)
        .map(|offset| bases[(start + offset) % len].clone())
        .collect()
}

/// 从 `start` 开始轮换的静态包地址。
fn rotated_sources(start: usize) -> Vec<(usize, String)> {
    rotated_bases(start)
        .into_iter()
        .map(|(index, prefix)| (index, format!("{prefix}{DIST_ARCHIVE_URL}")))
        .collect()
}

/// 上游 id -> 对外 id：去掉 `-free` 后缀；big-pickle 不对外暴露。
fn public_model_id(id: &str) -> Option<String> {
    if id == "big-pickle" {
        return None;
    }
    let base = id.strip_suffix("-free")?;
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

fn upstream_model_id(public: &str) -> String {
    format!("{public}-free")
}

fn extract_public_models(payload: &Value) -> Vec<String> {
    let mut ids: Vec<String> = payload["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str())
                .filter(|id| !id.starts_with("jev-"))
                .filter_map(public_model_id)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

fn read_models_cache(path: &PathBuf) -> Option<(Vec<String>, SystemTime)> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let stamp = value["fetchedAt"].as_u64()?;
    let ids: Vec<String> = value["models"]
        .as_array()?
        .iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect();
    if ids.is_empty() {
        return None;
    }
    let time = UNIX_EPOCH.checked_add(Duration::from_secs(stamp))?;
    Some((ids, time))
}

fn write_models_cache(path: &PathBuf, ids: &[String]) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let payload = json!({ "fetchedAt": stamp, "models": ids });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_vec_pretty(&payload).unwrap_or_default(),
    );
}

/// 启动器自己的设置（data_dir/settings.json）。
#[derive(Clone)]
struct LauncherSettings {
    /// 启动页语言（BCP-47 标签，例如 zh-Hans / zh-Hant / ja / ko / en）
    language: String,
    /// 昼夜主题（light / dark），空串 = 还没选过（跟随系统）
    theme: String,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            language: Lang::En.tag().to_string(),
            theme: String::new(),
        }
    }
}

fn read_settings(path: &PathBuf) -> LauncherSettings {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    LauncherSettings {
        language: value["language"]
            .as_str()
            .unwrap_or(Lang::En.tag())
            .to_string(),
        theme: value["theme"].as_str().unwrap_or("").to_string(),
    }
}

fn write_settings(path: &PathBuf, settings: LauncherSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = json!({
        "language": settings.language,
        "theme": settings.theme,
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&payload).unwrap_or_default(),
    )
    .map_err(|error| format!("保存启动器设置失败：{error}"))
}

/// 去掉 `Bearer ` 前缀（大小写不敏感），返回裸 Key。
fn strip_bearer(value: &str) -> &str {
    match value.trim().split_once(' ') {
        Some((scheme, key)) if scheme.eq_ignore_ascii_case("bearer") => key.trim(),
        _ => value.trim(),
    }
}

/// 判断调用方带来的 Key 是否是「看起来合法」的 opencode Key。
fn looks_like_api_key(value: &str) -> bool {
    let key = value.trim();
    if key.len() < 8 || key.len() > 200 {
        return false;
    }
    if matches!(
        key.to_ascii_lowercase().as_str(),
        "public" | "undefined" | "null" | "none" | "bearer"
    ) {
        return false;
    }
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '+' | '/' | '='))
}

fn session_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let stamp = now * 0x1000 + rand::thread_rng().gen_range(0..0x1000);
    let masked = !stamp;
    let mut hex = String::new();
    for shift in [40u32, 32, 24, 16, 8, 0] {
        hex.push_str(&format!("{:02x}", ((masked >> shift) & 0xff) as u8));
    }
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut tail = String::new();
    let mut rng = rand::thread_rng();
    for _ in 0..14 {
        tail.push(alphabet[rng.gen_range(0..alphabet.len())] as char);
    }
    format!("ses_{hex}{tail}")
}

fn read_tool() -> Value {
    json!({
        "type": "function",
        "function": {
            "name": "read",
            "description": "Read a file from the local filesystem.",
            "parameters": {
                "type": "object",
                "properties": {
                    "filePath": { "type": "string", "description": "The absolute path of the file to read." }
                },
                "required": ["filePath"]
            }
        }
    })
}

fn filter_response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let key = name.as_str();
        if matches!(
            key,
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "host"
                | "content-length"
        ) {
            continue;
        }
        out.insert(name.clone(), value.clone());
    }
    out
}

fn icon_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// 解析 `/icons/<path>`：先找本地化的游戏图标，再回落到工具箱静态目录里的同名图标。
fn resolve_icon(
    icons_dir: &std::path::Path,
    static_dir: &std::path::Path,
    path: &str,
) -> Option<PathBuf> {
    let relative = std::path::Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    let static_icons = static_dir.join("icons");
    for base in [icons_dir, static_icons.as_path()] {
        let candidate = base.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn icons_handler(
    State(state): State<Arc<ServerState>>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    let Some(file) = resolve_icon(&state.icons_dir, &state.static_dir, &path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response
                .headers_mut()
                .insert("content-type", HeaderValue::from_static(icon_mime(&file)));
            response.headers_mut().insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=300"),
            );
            response
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("读取图标失败：{error}"),
        )
            .into_response(),
    }
}

/// 工具箱自带的 `sw.js` / `registerSW.js` 一律换成空壳：
/// 静态文件本来就在本机，Service Worker 在这里只剩「把旧版本页面缓存住」一个作用。
async fn sw_handler() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SW_KILL,
    )
}

/// 静态产物的缓存策略：
/// - `_app/immutable/*` 和 `/icons/*` 文件名带内容哈希，可以长缓存；
/// - 其余（index.html / 200.html / manifest…）一律 `no-cache`，每次进工具箱都回源校验，
///   否则工具箱更新完 WebView2 还拿着旧页面，用户得自己按 Ctrl+Shift+R。
fn static_cache_value(path: &str) -> Option<&'static str> {
    if path.starts_with("/v1/") || path.starts_with("/api/") {
        return None;
    }
    Some(if path.starts_with("/_app/immutable/") || path.starts_with("/icons/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    })
}

async fn cache_control(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if let Some(value) = static_cache_value(&path) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    }
    response
}

async fn api_proxy(    State(state): State<Arc<ServerState>>,
    AxumPath(path): AxumPath<String>,
    RawQuery(query): RawQuery,
    request: axum::extract::Request,
) -> Response {
    let method = request.method().clone();

    // 带 lang 的请求是「同一路径的另一种语言」，不能拿中文缓存顶替，也不写进缓存。
    let localized = query
        .as_deref()
        .map(|value| value.to_ascii_lowercase().contains("lang="))
        .unwrap_or(false);

    if method == Method::GET && !localized {
        let key = format!("/api/{path}");
        if let Some(value) = state.game.read().await.entries.get(&key).cloned() {
            let mut response = Json(value).into_response();
            response.headers_mut().insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=60"),
            );
            return response;
        }
    }

    let headers = filter_response_headers(request.headers());
    let body = match to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(value) => value,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
    };

    let mut url = format!("{API_UPSTREAM}/api/{path}");
    if let Some(query) = query {
        url.push('?');
        url.push_str(&query);
    }

    let mut builder = state.client.request(method, url).headers(headers);
    if !body.is_empty() {
        builder = builder.body(body);
    }

    match builder.timeout(Duration::from_secs(30)).send().await {
        Ok(response) => {
            let status = response.status();
            let headers = filter_response_headers(response.headers());
            let stream = response.bytes_stream();
            let body = Body::from_stream(stream);
            let mut out = Response::new(body);
            *out.status_mut() = status;
            *out.headers_mut() = headers;
            out
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": { "message": format!("API 上游请求失败：{error}"), "type": "upstream_error" } })),
        )
            .into_response(),
    }
}

/// `Set-Cookie` 改写：去掉 `Domain=`，让 Cookie 落到 127.0.0.1 上（host-only）。
/// `Secure` / `SameSite` 不动：127.0.0.1 被视为可信来源，Secure 也能正常读写；
/// 反过来 `SameSite=None` 必须搭配 Secure，剥掉反而会失效。
fn rewrite_set_cookie(value: &str) -> String {
    value
        .split(';')
        .filter(|part| !part.trim().to_ascii_lowercase().starts_with("domain="))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("; ")
}

/// 3xx 的 Location：上游域换成反代地址。
fn rewrite_location(value: &str, base: &str) -> String {
    match value.strip_prefix(base) {
        Some(rest) => format!("http://127.0.0.1:{WORKSHOP_PORT}{rest}"),
        None => value.to_string(),
    }
}

/// 工坊反代：把任意工坊实例（用户填的地址）搬到 127.0.0.1:47822，使其与工具箱同站。
/// iframe 首次导航带 `?__coconut_target=<origin>` 指定上游，之后的绝对路径请求沿用它。
async fn workshop_proxy(State(state): State<Arc<ServerState>>, request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let incoming_headers = request.headers().clone();

    let mut target: Option<String> = None;
    let mut kept: Vec<String> = Vec::new();
    // 用 form-urlencoded 解析：目标地址是百分号编码过的
    for (key, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        if key == "__coconut_target" {
            target = Some(value.into_owned());
        } else {
            kept.push(
                url::form_urlencoded::Serializer::new(String::new())
                    .append_pair(&key, &value)
                    .finish(),
            );
        }
    }
    let kept_query = kept.join("&");

    if let Some(value) = target.as_deref() {
        // 只接受带协议头的 http(s) 地址
        if let Ok(parsed) = url::Url::parse(value) {
            if parsed.scheme() == "http" || parsed.scheme() == "https" {
                let origin = parsed.origin().ascii_serialization();
                *state.workshop_target.write().await = Some(origin);
            }
        }
    }

    let Some(base) = state.workshop_target.read().await.clone() else {
        return (
            StatusCode::BAD_GATEWAY,
            "工坊代理尚未指定上游：请从工具箱里打开工坊",
        )
            .into_response();
    };

    let mut url = format!("{base}{path}");
    if !kept_query.is_empty() {
        url.push('?');
        url.push_str(&kept_query);
    }

    let body = match to_bytes(request.into_body(), 64 * 1024 * 1024).await {
        Ok(value) => value,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
    };

    let mut headers = HeaderMap::new();
    for (name, value) in &incoming_headers {
        let key = name.as_str();
        if matches!(
            key,
            "host" | "origin" | "referer" | "content-length" | "connection" | "upgrade"
        ) {
            continue;
        }
        headers.insert(name.clone(), value.clone());
    }
    if let Ok(value) = HeaderValue::from_str(&base) {
        headers.insert("origin", value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("{base}{path}")) {
        headers.insert("referer", value);
    }

    let mut builder = state.client.request(method, &url).headers(headers);
    if !body.is_empty() {
        builder = builder.body(body);
    }

    match builder.timeout(Duration::from_secs(60)).send().await {
        Ok(response) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            let mut out = Response::new(Body::from_stream(response.bytes_stream()));
            *out.status_mut() = status;
            let target_headers = out.headers_mut();
            for (name, value) in &upstream_headers {
                let key = name.as_str();
                match key {
                    "set-cookie" => {
                        if let Ok(text) = value.to_str() {
                            if let Ok(header) = HeaderValue::from_str(&rewrite_set_cookie(text)) {
                                target_headers.append("set-cookie", header);
                            }
                        }
                    }
                    "location" => {
                        if let Ok(text) = value.to_str() {
                            if let Ok(header) = HeaderValue::from_str(&rewrite_location(text, &base))
                            {
                                target_headers.append("location", header);
                            }
                        }
                    }
                    // 换源之后这些头要么失效要么会拦自己
                    "content-length"
                    | "connection"
                    | "keep-alive"
                    | "transfer-encoding"
                    | "content-security-policy"
                    | "content-security-policy-report-only"
                    | "strict-transport-security"
                    | "x-frame-options" => {}
                    _ => {
                        target_headers.append(name.clone(), value.clone());
                    }
                }
            }
            out
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("工坊上游请求失败：{error}"),
        )
            .into_response(),
    }
}

async fn models_handler(State(state): State<Arc<ServerState>>) -> Response {
    let ids = state.model_ids(false).await;
    let data: Vec<Value> = ids
        .iter()
        .map(|id| json!({ "id": id, "object": "model", "owned_by": "opencode" }))
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}

fn upstream_headers(auth: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "accept",
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    if let Ok(value) = HeaderValue::from_str(auth) {
        headers.insert("authorization", value);
    } else {
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer public"),
        );
    }
    headers.insert("user-agent", HeaderValue::from_static(USER_AGENT));
    let session = session_id();
    if let Ok(value) = HeaderValue::from_str(&session) {
        headers.insert(HeaderName::from_static("x-opencode-session"), value.clone());
        headers.insert(HeaderName::from_static("x-opencode-session-id"), value);
    }
    headers
}

async fn chat_handler(
    State(state): State<Arc<ServerState>>,
    request: axum::extract::Request,
) -> Response {
    let caller_auth = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = match to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(value) => value,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
    };
    let mut payload: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": { "message": "invalid JSON body", "type": "invalid_request_error" } })),
            )
                .into_response()
        }
    };

    if !payload
        .get("messages")
        .map(Value::is_array)
        .unwrap_or(false)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": { "message": "messages must be an array", "type": "invalid_request_error" } })),
        )
            .into_response();
    }

    let requested = payload["model"]
        .as_str()
        .unwrap_or("")
        .split('/')
        .last()
        .unwrap_or("")
        .to_string();
    // 兼容还没同步过的旧配置：调用方仍然带着 -free 后缀。
    let requested = requested
        .strip_suffix("-free")
        .unwrap_or(&requested)
        .to_string();
    let ids = state.model_ids(false).await;
    if ids.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": { "message": "免费模型列表为空", "type": "upstream_error" } })),
        )
            .into_response();
    }
    let target = if ids.contains(&requested) {
        requested.clone()
    } else {
        ids[0].clone()
    };
    let auth = state.client_auth(caller_auth.as_deref()).await;
    let mut candidates = vec![target.clone()];
    for id in ids.iter() {
        if id != &target {
            candidates.push(id.clone());
        }
    }

    let caller_tools = payload["tools"].as_array().cloned().unwrap_or_default();
    let has_read = caller_tools
        .iter()
        .any(|tool| tool.pointer("/function/name").and_then(Value::as_str) == Some("read"));
    let bridge = !has_read;
    let mut tools = caller_tools.clone();
    if bridge {
        tools.push(read_tool());
    }
    let caller_has_tools = !caller_tools.is_empty();
    let wants_stream = payload["stream"].as_bool().unwrap_or(false);

    let mut last_status = StatusCode::BAD_GATEWAY;
    let mut last_message = String::new();
    let mut chosen: Option<(reqwest::Response, String)> = None;

    for model in candidates {
        // 对调用方隐藏 -free 后缀，转发时补回上游真实模型名。
        payload["model"] = json!(upstream_model_id(&model));
        payload["stream"] = json!(true);
        if !tools.is_empty() {
            payload["tools"] = json!(tools);
        } else {
            payload.as_object_mut().map(|object| object.remove("tools"));
        }
        if !caller_has_tools && bridge {
            payload["tool_choice"] = json!("none");
        }

        let response = state
            .client
            .post(format!("{UPSTREAM}/chat/completions"))
            .headers(upstream_headers(&auth))
            .json(&payload)
            .send()
            .await;

        match response {
            Err(error) => {
                last_status = StatusCode::BAD_GATEWAY;
                last_message = error.to_string();
            }
            Ok(value) if value.status().is_success() => {
                chosen = Some((value, model));
            }
            Ok(value) => {
                last_status = value.status();
                last_message = value.text().await.unwrap_or_default();
            }
        }

        if chosen.is_some() {
            break;
        }
    }

    let (upstream, served_by) = match chosen {
        Some(value) => value,
        None => {
            return (
                last_status,
                Json(json!({
                    "error": {
                        "message": format!("All free models failed: {last_message}"),
                        "type": "upstream_error"
                    }
                })),
            )
                .into_response()
        }
    };

    let requested_model = requested.clone();
    let fallback = served_by != requested_model;
    let strip_read = bridge;

    if wants_stream {
        let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(32);
        tokio::spawn(relay_stream(upstream, tx, requested_model, strip_read));
        let body = Body::from_stream(ReceiverStream::new(rx));
        let mut response = Response::new(body);
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-cache"));
        response.headers_mut().insert(
            "x-coconut-served-by",
            HeaderValue::from_str(&served_by).unwrap_or(HeaderValue::from_static("")),
        );
        if fallback {
            response
                .headers_mut()
                .insert("x-coconut-fallback", HeaderValue::from_static("true"));
        }
        response
    } else {
        let completion = collect_stream(upstream, requested_model, strip_read).await;
        Json(completion).into_response()
    }
}

fn transform_event(
    mut event: Value,
    requested: &str,
    strip_read: bool,
    suppressed: &mut HashSet<usize>,
    has_caller_tool_call: &mut bool,
) -> Value {
    if event.get("model").is_some() {
        event["model"] = json!(requested);
    }
    if let Some(choices) = event.get_mut("choices").and_then(Value::as_array_mut) {
        for choice in choices {
            let mut finish = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) {
                if let Some(calls) = delta.get_mut("tool_calls").and_then(Value::as_array_mut) {
                    let mut kept = Vec::new();
                    for call in calls.iter() {
                        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if strip_read && name == "read" {
                            suppressed.insert(index);
                            continue;
                        }
                        if suppressed.contains(&index) {
                            continue;
                        }
                        *has_caller_tool_call = true;
                        kept.push(call.clone());
                    }
                    if kept.is_empty() {
                        delta.remove("tool_calls");
                    } else {
                        *calls = kept;
                    }
                }
            }
            if finish == "tool_calls" && !*has_caller_tool_call {
                choice["finish_reason"] = json!("stop");
                finish = "stop".to_string();
            }
            let _ = finish;
        }
    }
    event
}

async fn relay_stream(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<Bytes, Infallible>>,
    requested: String,
    strip_read: bool,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut suppressed = HashSet::new();
    let mut has_caller = false;

    let send = |text: String| {
        let tx = tx.clone();
        async move {
            let _ = tx.send(Ok(Bytes::from(text))).await;
        }
    };

    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        buffer = buffer.replace("\r\n", "\n");
        while let Some(index) = buffer.find("\n\n") {
            let block = buffer[..index].to_string();
            buffer = buffer[index + 2..].to_string();
            let data = block
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                send("data: [DONE]\n\n".into()).await;
                return;
            }
            if let Ok(event) = serde_json::from_str::<Value>(&data) {
                let event = transform_event(
                    event,
                    &requested,
                    strip_read,
                    &mut suppressed,
                    &mut has_caller,
                );
                send(format!(
                    "data: {}\n\n",
                    serde_json::to_string(&event).unwrap_or_default()
                ))
                .await;
            }
        }
    }
    send("data: [DONE]\n\n".into()).await;
}

async fn collect_stream(response: reqwest::Response, requested: String, strip_read: bool) -> Value {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut suppressed = HashSet::new();
    let mut has_caller = false;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut finish = "stop".to_string();
    let mut usage = Value::Null;
    let mut calls: HashMap<usize, Value> = HashMap::new();

    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            break;
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        buffer = buffer.replace("\r\n", "\n");
        while let Some(index) = buffer.find("\n\n") {
            let block = buffer[..index].to_string();
            buffer = buffer[index + 2..].to_string();
            let data = block
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Value>(&data) {
                let event = transform_event(
                    event,
                    &requested,
                    strip_read,
                    &mut suppressed,
                    &mut has_caller,
                );
                if let Some(value) = event.get("usage") {
                    if !value.is_null() {
                        usage = value.clone();
                    }
                }
                for choice in event["choices"].as_array().cloned().unwrap_or_default() {
                    let delta = &choice["delta"];
                    if let Some(text) = delta["content"].as_str() {
                        content.push_str(text);
                    }
                    if let Some(text) = delta["reasoning_content"].as_str() {
                        reasoning.push_str(text);
                    }
                    for call in delta["tool_calls"].as_array().cloned().unwrap_or_default() {
                        let index = call["index"].as_u64().unwrap_or(0) as usize;
                        let entry = calls.entry(index).or_insert_with(|| {
                            json!({
                                "id": call["id"].as_str().unwrap_or(""),
                                "type": "function",
                                "function": { "name": "", "arguments": "" }
                            })
                        });
                        if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                            if entry["function"]["name"].as_str().unwrap_or("").is_empty() {
                                entry["function"]["name"] = json!(name);
                            }
                        }
                        if let Some(args) =
                            call.pointer("/function/arguments").and_then(Value::as_str)
                        {
                            let current = entry["function"]["arguments"].as_str().unwrap_or("");
                            entry["function"]["arguments"] = json!(format!("{current}{args}"));
                        }
                    }
                    if let Some(reason) = choice["finish_reason"].as_str() {
                        finish = reason.to_string();
                    }
                }
            }
        }
    }

    let mut ordered: Vec<(usize, Value)> = calls.into_iter().collect();
    ordered.sort_by_key(|(index, _)| *index);
    let tool_calls: Vec<Value> = ordered.into_iter().map(|(_, value)| value).collect();
    let message = if tool_calls.is_empty() {
        json!({ "role": "assistant", "content": content, "reasoning_content": reasoning })
    } else {
        json!({ "role": "assistant", "content": Value::Null, "reasoning_content": reasoning, "tool_calls": tool_calls })
    };
    json!({
        "id": format!("chatcmpl-{}", rand::thread_rng().gen::<u64>()),
        "object": "chat.completion",
        "created": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        "model": requested,
        "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
        "usage": usage
    })
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<ServerState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<ServerState>) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let session = Arc::new(WsSession {
        id: format!("{}", rand::thread_rng().gen::<u64>()),
        tx: tx.clone(),
        pending: Mutex::new(HashMap::new()),
        tools: Mutex::new(HashSet::new()),
        closed: AtomicBool::new(false),
        frames: state.frames.clone(),
    });
    state.ws.add(session.clone()).await;
    state.log("ws", &format!("会话建立 {}", session.id));

    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = receiver.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let _ = session.frames.send(json!({
            "direction": "recv",
            "frame": value,
            "at": now_millis(),
        }));
        match value["type"].as_str().unwrap_or("") {
            "ping" => {
                let _ = tx.send(json!({ "type": "pong" }).to_string());
            }
            "log" => {
                let source = value["source"].as_str().unwrap_or("plugins");
                let limit = value["limit"].as_u64().unwrap_or(400).min(2000) as usize;
                let lines = state.log_tail(source, limit);
                let _ = tx.send(
                    json!({
                        "type": "log-lines",
                        "source": source,
                        "lines": lines,
                        "at": now_millis(),
                    })
                    .to_string(),
                );
            }
            "hello" => {
                let tools: HashSet<String> = value["tools"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|tool| {
                                tool.pointer("/function/name").and_then(Value::as_str)
                            })
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                *session.tools.lock().await = tools;
                *state.page_session.write().await = Some(session.clone());
                state.log(
                    "ws",
                    &format!("hello {}（{} 个工具）", session.id, session.tools.lock().await.len()),
                );
                let state = state.clone();
                let session = session.clone();
                let session_id = session.id.clone();
                let tool_names: Vec<String> =
                    session.tools.lock().await.iter().cloned().collect();
                // 只有真正的工具箱页面（有 run_js）才算「页面就绪」：
                // 沉浸式外壳自己开的那条 WS 也会发 hello，它一触发插件同步就会
                // 因为「工具箱页面还没打开」白跑一轮。
                tokio::spawn(async move {
                    // 主题透传 + 沉浸式外壳还是启动器自己的事；
                    // 免费模型等业务逻辑改成插件，走 onHello 事件。
                    apply_theme(&state, &session).await;
                    // 页面刚起时注入可能因为还在 hydration 而失败，重试几次，
                    // 否则插件永远不会收到 onHello（它只在注入成功后触发）。
                    let mut injected = false;
                    for attempt in 0..3 {
                        if attempt > 0 {
                            tokio::time::sleep(Duration::from_millis(1500)).await;
                            if session.closed.load(Ordering::SeqCst) {
                                return;
                            }
                        }
                        if inject_shell(state.clone(), session.clone()).await {
                            injected = true;
                            break;
                        }
                    }
                    // 注入成功 = 页面上的 run_js 确实可用，这时候再叫插件干活，
                    // 否则插件会因为「工具箱页面还没打开」白跑一轮。
                    if injected {
                        // 页面刚注入完往往还在 hydration / 重新挂载，立即调用可能撞上
                        // 「会话刚要关」的瞬间。先发一次，4 秒后再补一次，插件那侧还有
                        // 自己的重试兜底。
                        state.plugins.hello(json!({
                            "session": session_id,
                            "tools": tool_names,
                        }));
                        let retry_state = state.clone();
                        let retry_id = session.id.clone();
                        let retry_tools = tool_names.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(4)).await;
                            retry_state.plugins.hello(json!({
                                "session": retry_id,
                                "tools": retry_tools,
                            }));
                        });
                    }
                });
            }
            "window" => {
                let reply = state.window_action(&value).await;
                let _ = tx.send(reply.to_string());
            }
            "result" => {
                if let Some(id) = value["id"].as_str() {
                    if let Some(tx) = session.pending.lock().await.remove(id) {
                        let _ = tx.send(value);
                    }
                }
            }
            _ => {}
        }
    }

    session.closed.store(true, Ordering::SeqCst);
    state.ws.remove(&session.id).await;
    state.log("ws", &format!("会话关闭 {}", session.id));
    writer.abort();
}

/// 注入沉浸式外壳。返回是否注入成功——成功后 `run_js` 一定可用，
/// 插件同步（免费模型）才会被触发。
async fn inject_shell(state: Arc<ServerState>, session: Arc<WsSession>) -> bool {
    if !session.has_tool_async("run_js").await {
        return false;
    }
    match session
        .exec(
            "run_js",
            json!({ "code": SHELL_INJECT }),
            Duration::from_secs(20),
        )
        .await
    {
        Ok(value) => {
            let detail = value
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_string();
            state
                .log("shell", &format!("沉浸式外壳注入完成：{detail}"));
            true
        }
        Err(error) => {
            state.log("shell", &format!("沉浸式外壳注入失败：{error}"));
            false
        }
    }
}

/// 免费模型配置的备注名：所有托管配置共用 `OC_FREE`（模型名放在 `model` 字段里），
/// 前后各带一个零宽字符，用来在工具箱里认出「这是启动器写的配置」。
/// 主题透传：工具箱的 theme_mode 跟着启动器走。
async fn apply_theme(state: &Arc<ServerState>, session: &Arc<WsSession>) {
    let theme = state.theme.read().await.clone();
    if theme.is_empty() {
        return;
    }
    if session.has_tool_async("set_setting").await {
        let _ = session
            .exec(
                "set_setting",
                json!({ "key": "theme_mode", "value": theme }),
                Duration::from_secs(10),
            )
            .await;
    }
}

async fn refresh_models(state: Arc<ServerState>) {
    let mut previous = state.model_ids(false).await;
    loop {
        // 一天刷新一次；列表为空（抓取失败）时每 10 分钟重试。
        let wait = if previous.is_empty() {
            MODELS_RETRY
        } else {
            MODELS_TTL
        };
        tokio::time::sleep(wait).await;
        let next = state.model_ids(true).await;
        if !next.is_empty() && next != previous {
            previous = next;
            state.ws.sync_all(state.clone()).await;
        }
    }
}

async fn fetch_remote_json(client: &reqwest::Client, path: &str) -> Result<Value, String> {
    let url = format!("{API_UPSTREAM}{path}");
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| format!("请求失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response
        .json::<Value>()
        .await
        .map_err(|error| format!("响应不是 JSON：{error}"))
}

fn encode_name(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn list_names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 给 panic hook 用的毫秒时间戳。
pub fn now_millis_public() -> u64 {
    now_millis()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 插件宿主专用的 tokio runtime。
///
/// 插件线程是普通线程，不能借用启动器 / Tauri 的 runtime：那边是 current_thread
/// 或者拿不到 IO 驱动，future 一用到计时器就会 panic「there is no reactor running」。
/// 自己养一个 2 线程的 runtime，任务和连接都落在它上面。
fn bridge_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("coconut-plugin-host")
            .build()
            .expect("创建插件宿主 runtime 失败")
    })
}

/// 在普通线程上阻塞等待一个 tokio future（future 必须在 runtime 内部创建）。
fn bridge_block_on<F, Fut>(build: F) -> Fut::Output
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future,
{
    // 关键：`build()` 必须在 `block_on` 的 future 内部调用。
    // 写成 `block_on(build())` 会在进入 runtime 之前构造 future，
    // reqwest 这类在构造期取 `Handle::current()`，会直接 panic「no reactor running」。
    bridge_runtime().block_on(async move { build().await })
}

/// 插件宿主：把插件要的能力接到现有的 WS / reqwest / 磁盘上。
struct ServerBridge {
    state: std::sync::Weak<ServerState>,
}

impl ServerBridge {
    fn state(&self) -> Result<Arc<ServerState>, String> {
        self.state
            .upgrade()
            .ok_or_else(|| "启动器正在退出".to_string())
    }

    /// 找一条支持该工具的最新的 WS 会话（一般就是工具箱页面自己）。
    fn session(&self, state: &Arc<ServerState>, tool: &str) -> Result<Arc<WsSession>, String> {
        let log_state = state.clone();
        // 先看 hub；hub 里没有（页面刚建 / 刚要关的瞬间）就退回「最后一次就绪的页面会话」。
        let remembered = bridge_block_on(|| {
            let state = state.clone();
            async move { state.page_session.read().await.clone() }
        });
        let snapshot = bridge_block_on(|| {
            let state = state.clone();
            async move { state.ws.snapshot().await }
        });
        let mut candidates: Vec<(Arc<WsSession>, HashSet<String>)> = Vec::new();
        for session in snapshot {
            let tools = bridge_block_on(|| {
                let session = session.clone();
                async move { session.tools.lock().await.clone() }
            });
            candidates.push((session, tools));
        }
        if let Some(session) = remembered {
            if !candidates.iter().any(|(hit, _)| Arc::ptr_eq(hit, &session)) {
                let tools = bridge_block_on(|| {
                    let session = session.clone();
                    async move { session.tools.lock().await.clone() }
                });
                candidates.push((session, tools));
            }
        }
        for (session, tools) in candidates.iter().rev() {
            // 只挑还活着的连接：页面刷新后旧会话会挂在这里，
            // 挑到它就会报「WS 连接已关闭」，插件还得再等一轮。
            if session.closed.load(Ordering::SeqCst) {
                continue;
            }
            if tools.contains(tool) {
                return Ok(session.clone());
            }
        }
        let report: Vec<String> = candidates
            .iter()
            .map(|(session, tools)| {
                format!(
                    "{}:closed={},tools={},has={}",
                    session.id,
                    session.closed.load(Ordering::SeqCst),
                    tools.len(),
                    tools.contains(tool)
                )
            })
            .collect();
        eprintln!(
            "[plugin] 找不到带 {tool} 的会话：{}",
            if report.is_empty() {
                "（没有任何 WS 会话）".to_string()
            } else {
                report.join(" ")
            }
        );
        log_state.log(
            "plugin-bridge",
            &format!(
                "找不到带 {tool} 的会话：{}",
                if report.is_empty() {
                    "（没有任何 WS 会话）".to_string()
                } else {
                    report.join(" ")
                }
            ),
        );
        Err("工具箱页面还没打开".to_string())
    }

    fn exec(&self, state: &Arc<ServerState>, tool: &str, args: Value) -> Result<Value, String> {
        let session = self.session(state, tool)?;
        bridge_block_on(|| session.exec(tool, args, Duration::from_secs(30)))
    }
}

/// 跑一段页面 JS，拿回 run_js 的 value（exec 给的是 {value, logs}）。
fn run_js_value(
    bridge: &ServerBridge,
    state: &Arc<ServerState>,
    script: &str,
) -> Result<Value, String> {
    let payload = bridge.exec(state, "run_js", json!({ "code": script }))?;
    Ok(payload.get("value").cloned().unwrap_or(payload))
}

impl PluginBridge for ServerBridge {
    fn log(&self, plugin: &str, level: &str, message: &str) {
        if let Ok(state) = self.state() {
            state
                .log(&format!("plugin:{plugin}"), &format!("[{level}] {message}"));
            state
                .plugins
                .push_log(format!("[{plugin}] {message}"));
        }
    }

    fn run_js(&self, _plugin: &str, code: &str) -> Result<String, String> {
        let state = self.state()?;
        let payload = self.exec(&state, "run_js", json!({ "code": code }))?;
        // exec 给的是 data（{value,logs}），run_js 的真正返回值在 value 里
        let value = payload.get("value").cloned().unwrap_or(payload);
        Ok(match value {
            Value::String(text) => text,
            Value::Null => "null".to_string(),
            other => other.to_string(),
        })
    }

    fn call_tool(&self, _plugin: &str, tool: &str, args: &str) -> Result<String, String> {
        let state = self.state()?;
        let args = serde_json::from_str::<Value>(args).unwrap_or_else(|_| json!({}));
        let value = self.exec(&state, tool, args)?;
        Ok(value.to_string())
    }

    fn fetch(&self, _plugin: &str, url: &str, opts: &str) -> Result<String, String> {
        let state = self.state()?;
        let opts = serde_json::from_str::<Value>(opts).unwrap_or_else(|_| json!({}));
        let method = opts["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| format!("非法的 HTTP 方法：{method}"))?;
        let mut request = state
            .client
            .request(method, url)
            .timeout(Duration::from_secs(30));
        if let Some(headers) = opts["headers"].as_object() {
            for (key, value) in headers {
                if let Some(value) = value.as_str() {
                    request = request.header(key.as_str(), value);
                }
            }
        }
        if let Some(body) = opts["body"].as_str() {
            request = request.body(body.to_string());
        } else if let Some(value) = opts.get("json") {
            if !value.is_null() {
                request = request.json(value);
            }
        }
        let response = bridge_block_on(|| request.send())
            .map_err(|error| format!("请求失败：{error}"))?;
        let status = response.status().as_u16();
        let ok = response.status().is_success();
        let headers: serde_json::Map<String, Value> = response
            .headers()
            .iter()
            .map(|(key, value)| {
                (
                    key.as_str().to_string(),
                    json!(value.to_str().unwrap_or("")),
                )
            })
            .collect();
        let body = bridge_block_on(|| response.text())
            .map_err(|error| format!("读取响应失败：{error}"))?;
        Ok(json!({
            "status": status,
            "ok": ok,
            "headers": Value::Object(headers),
            "body": body,
            "json": serde_json::from_str::<Value>(&body).unwrap_or(Value::Null),
        })
        .to_string())
    }

    fn ui_overlay(&self, plugin: &str, html: &str, css: &str) -> Result<u32, String> {
        let state = self.state()?;
        // run_js 的 code 是「函数体」：必须自己 return，工具箱才会把值带回来。
        let script = format!(
            "return globalThis.__coconutOverlayRegister ? globalThis.__coconutOverlayRegister({}, {}) : 0",
            json!(plugin),
            json!({ "html": html, "css": css })
        );
        let value = run_js_value(self, &state, &script)?;
        let id = value
            .as_u64()
            .or_else(|| value.as_str().and_then(|raw| raw.trim().parse::<u64>().ok()))
            .ok_or_else(|| "浮层注册失败：工具箱页面没有返回 id".to_string())?;
        Ok(id as u32)
    }

    fn ui_overlay_visible(&self, plugin: &str, id: u32, visible: bool) -> Result<bool, String> {
        let state = self.state()?;
        let script = format!(
            "return globalThis.__coconutOverlayVisible ? globalThis.__coconutOverlayVisible({}, {}) : false",
            json!(id),
            json!(visible)
        );
        let _ = plugin;
        let value = run_js_value(self, &state, &script)?;
        Ok(value
            .as_bool()
            .or_else(|| value.as_str().map(|raw| raw.trim() == "true"))
            .unwrap_or(false))
    }

    fn ui_overlay_remove(&self, plugin: &str, id: u32) -> Result<bool, String> {
        let state = self.state()?;
        let script = format!(
            "return globalThis.__coconutOverlayRemove ? globalThis.__coconutOverlayRemove({}) : false",
            json!(id)
        );
        let _ = plugin;
        let value = run_js_value(self, &state, &script)?;
        Ok(value
            .as_bool()
            .or_else(|| value.as_str().map(|raw| raw.trim() == "true"))
            .unwrap_or(false))
    }

    fn ui_inject_css(&self, plugin: &str, id: &str, css: &str) -> Result<(), String> {
        let state = self.state()?;
        let script = format!(
            "globalThis.__coconutInjectCss && globalThis.__coconutInjectCss({}, {}, {})",
            json!(plugin),
            json!(id),
            json!(css)
        );
        self.exec(&state, "run_js", json!({ "code": script }))
            .map(|_| ())
    }

    fn launcher_info(&self) -> Result<Value, String> {
        let state = self.state()?;
        let snapshot = state.clone();
        let (lang, game_version) = bridge_block_on(move || async move {
            (
                snapshot.lang().await.tag().to_string(),
                snapshot.game.read().await.version.clone(),
            )
        });
        let commit = state
            .commit
            .try_read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let build = state
            .build
            .try_read()
            .map(|value| value.clone())
            .unwrap_or_default();
        let toolbox_version = if commit.trim().is_empty() {
            build.trim().to_string()
        } else {
            commit.trim().to_string()
        };
        Ok(json!({
            "lang": lang,
            "toolboxVersion": toolbox_version,
            "gameVersion": game_version,
            "src": {
                "toolbox": {
                    "dir": state.static_dir.display().to_string(),
                    "url": format!("http://127.0.0.1:{TOOLBOX_PORT}/"),
                },
                "static": {
                    "dir": state.icons_dir.display().to_string(),
                    "url": format!("http://127.0.0.1:{TOOLBOX_PORT}/icons/"),
                },
            }
        }))
    }

    fn storage_get(&self, plugin: &str, key: &str) -> Result<String, String> {
        let state = self.state()?;
        Ok(state.plugins.storage_get(plugin, key))
    }

    fn storage_set(&self, plugin: &str, key: &str, value: &str) -> Result<(), String> {
        let state = self.state()?;
        state.plugins.storage_set(plugin, key, value);
        Ok(())
    }

    fn storage_delete(&self, plugin: &str, key: &str) -> Result<(), String> {
        let state = self.state()?;
        state.plugins.storage_delete(plugin, key);
        Ok(())
    }

    fn storage_all(&self, plugin: &str) -> Result<String, String> {
        let state = self.state()?;
        Ok(state.plugins.storage_all(plugin))
    }
}

pub async fn check_game_update(state: &ServerState) -> Result<Value, String> {    let lang = state.lang().await;
    state
        .set_progress(tr!(
            lang,
            "正在检查游戏数据版本…",
            "正在檢查遊戲資料版本…",
            "ゲームデータ版を確認中…",
            "게임 데이터 버전 확인 중…",
            "Checking the game data version…"
        ))
        .await;
    let remote = fetch_remote_json(&state.client, "/api/v1/version/latest").await?;
    let remote = remote.as_str().unwrap_or("").trim().to_string();
    if remote.is_empty() {
        return Err("上游没有返回游戏数据版本".into());
    }
    let local = state.game.read().await.version.clone();
    state
        .set_progress(if local == remote {
            format!(
                "{} · v{local}",
                tr!(
                    lang,
                    "游戏数据已是最新",
                    "遊戲資料已是最新",
                    "ゲームデータは最新です",
                    "게임 데이터가 최신입니다",
                    "Game data is up to date"
                )
            )
        } else {
            format!(
                "{} · v{remote}",
                tr!(
                    lang,
                    "发现游戏数据新版本",
                    "發現遊戲資料新版本",
                    "ゲームデータの新バージョン",
                    "게임 데이터 새 버전 발견",
                    "New game data version"
                )
            )
        })
        .await;
    Ok(json!({
        "available": local != remote,
        "local": local,
        "remote": remote,
    }))
}

/// 列表类路径：可用版本 + 4 个列表 + 4 个图标表（图标会本地化成 webp）。
const GAME_LIST_PATHS: &[&str] = &[
    "/api/v1/version/available",
    "/api/v1/list/character",
    "/api/v1/list/weapon",
    "/api/v1/list/echo",
    "/api/v1/list/echo-set",
    "/api/v1/batch-icons/character",
    "/api/v1/batch-icons/weapon",
    "/api/v1/batch-icons/echo",
    "/api/v1/batch-icons/echo-set",
];

fn is_entity_path(path: &str) -> bool {
    path.starts_with("/api/v1/info/")
        || path.starts_with("/api/v2/info/")
        || path.starts_with("/api/v3/info/")
        || path.starts_with("/api/v1/recommend-weapon/")
}

/// 由列表展开出需要逐条拉取的实体路径。
fn entity_paths(entries: &HashMap<String, Value>) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in GAME_LIST_PATHS {
        if !path.starts_with("/api/v1/list/") {
            continue;
        }
        let Some(value) = entries.get(*path) else {
            continue;
        };
        let entity = path.rsplit('/').next().unwrap_or("");
        for name in list_names(value) {
            let encoded = encode_name(&name);
            let mut push = |candidate: String| {
                if seen.insert(candidate.clone()) {
                    paths.push(candidate);
                }
            };
            push(format!("/api/v1/info/{entity}/{encoded}"));
            if entity == "character" {
                push(format!("/api/v2/info/character/{encoded}"));
                push(format!("/api/v3/info/character/{encoded}"));
                push(format!("/api/v1/recommend-weapon/{encoded}"));
            }
        }
    }
    paths
}

async fn fetch_game_lists(
    state: &Arc<ServerState>,
) -> Result<(String, HashMap<String, Value>), String> {
    let lang = state.lang().await;
    state
        .set_progress(tr!(
            lang,
            "正在读取游戏数据版本…",
            "正在讀取遊戲資料版本…",
            "ゲームデータ版を読み込み中…",
            "게임 데이터 버전 읽는 중…",
            "Reading the game data version…"
        ))
        .await;
    let version = fetch_remote_json(&state.client, "/api/v1/version/latest").await?;
    let version = version.as_str().unwrap_or("").trim().to_string();
    if version.is_empty() {
        return Err("上游没有返回游戏数据版本".into());
    }

    let mut entries: HashMap<String, Value> = HashMap::new();
    entries.insert("/api/v1/version/latest".to_string(), json!(version));

    state
        .set_progress(tr!(
            lang,
            "正在读取游戏数据列表…",
            "正在讀取遊戲資料清單…",
            "ゲームデータ一覧を読み込み中…",
            "게임 데이터 목록 읽는 중…",
            "Reading the game data lists…"
        ))
        .await;
    for path in GAME_LIST_PATHS {
        let mut value = fetch_remote_json(&state.client, path).await?;
        if path.starts_with("/api/v1/batch-icons/") {
            value = localize_icons(state, &value).await;
        }
        entries.insert((*path).to_string(), value);
    }
    Ok((version, entries))
}

async fn fetch_entity_paths(
    state: &Arc<ServerState>,
    paths: &[String],
    entries: &mut HashMap<String, Value>,
    label: &str,
) {
    let total = paths.len();
    if total == 0 {
        return;
    }
    state
        .set_progress(format!("{label} 0/{total}…"))
        .await;
    let done = Arc::new(AtomicUsize::new(0));
    let jobs = paths.iter().cloned().map(|path| {
        let client = state.client.clone();
        let state = state.clone();
        let done = done.clone();
        let label = label.to_string();
        async move {
            let result = fetch_remote_json(&client, &path).await;
            let current = done.fetch_add(1, Ordering::SeqCst) + 1;
            if current == total || current % 12 == 0 {
                state
                    .set_progress(format!("{label} {current}/{total}…"))
                    .await;
            }
            (path, result)
        }
    });
    let results = futures_util::stream::iter(jobs)
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    for (key, result) in results {
        if let Ok(value) = result {
            entries.insert(key, value);
        }
    }
}

/// 图标本地化：下载 → 解码 → 重新编码成无损 webp 落盘，返回本地 URL。
fn hash_url(url: &str) -> String {
    // FNV-1a 64：稳定、无需额外依赖。
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn transcode_webp(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|error| format!("图片解码失败：{error}"))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(
            image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| format!("webp 编码失败：{error}"))?;
    Ok(out)
}

async fn ensure_icon(state: &Arc<ServerState>, url: &str) -> Result<String, String> {
    let name = format!("{}.webp", hash_url(url));
    let path = state.icons_dir.join(&name);
    let local = format!("http://127.0.0.1:{TOOLBOX_PORT}/icons/{name}");
    if std::fs::metadata(&path).map(|meta| meta.len() > 0).unwrap_or(false) {
        return Ok(local);
    }
    let response = state
        .client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| format!("下载图标失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("图标 HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("读取图标失败：{error}"))?;
    let webp = transcode_webp(&bytes)?;
    std::fs::create_dir_all(&state.icons_dir).map_err(|error| format!("创建图标目录失败：{error}"))?;
    let tmp = path.with_extension("webp.tmp");
    std::fs::write(&tmp, &webp).map_err(|error| format!("写入图标失败：{error}"))?;
    std::fs::rename(&tmp, &path).map_err(|error| format!("替换图标失败：{error}"))?;
    Ok(local)
}

async fn localize_icons(state: &Arc<ServerState>, value: &Value) -> Value {
    let Some(map) = value.as_object() else {
        return value.clone();
    };
    let pairs: Vec<(String, String)> = map
        .iter()
        .filter_map(|(name, url)| url.as_str().map(|url| (name.clone(), url.to_string())))
        .collect();
    let total = pairs.len();
    if total == 0 {
        return value.clone();
    }
    let lang = state.lang().await;
    state
        .set_progress(format!(
            "{} 0/{total}…",
            tr!(
                lang,
                "正在缓存图标",
                "正在快取圖示",
                "アイコンをキャッシュ中",
                "아이콘 캐시 중",
                "Caching icons"
            )
        ))
        .await;
    let done = Arc::new(AtomicUsize::new(0));
    let tasks = pairs.into_iter().map(|(name, url)| {
        let state = state.clone();
        let done = done.clone();
        async move {
            let local = match ensure_icon(&state, &url).await {
                Ok(path) => path,
                Err(error) => {
                    state.log("icons", &format!("图标本地化失败：{url}（{error}），保留远程地址"));
                    url.clone()
                }
            };
            let current = done.fetch_add(1, Ordering::SeqCst) + 1;
            if current == total || current % 20 == 0 {
                state
                    .set_progress(format!("正在缓存图标 {current}/{total}…"))
                    .await;
            }
            (name, Value::String(local))
        }
    });
    let results = futures_util::stream::iter(tasks)
        .buffer_unordered(6)
        .collect::<Vec<_>>()
        .await;
    let mut out = serde_json::Map::new();
    for (name, value) in results {
        out.insert(name, value);
    }
    Value::Object(out)
}

fn stamp_now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

async fn save_game_data(state: &Arc<ServerState>, data: &GameData) -> Result<(), String> {
    let lang = state.lang().await;
    state
        .set_progress(tr!(
            lang,
            "正在写入本地游戏数据…",
            "正在寫入本機遊戲資料…",
            "ゲームデータを保存中…",
            "게임 데이터 저장 중…",
            "Saving game data locally…"
        ))
        .await;
    let encoded = serde_json::to_vec(data).map_err(|error| error.to_string())?;
    let tmp = state.game_file.with_extension("json.tmp");
    tokio::fs::write(&tmp, encoded)
        .await
        .map_err(|error| format!("写入游戏数据失败：{error}"))?;
    if tokio::fs::try_exists(&state.game_file)
        .await
        .unwrap_or(false)
    {
        tokio::fs::remove_file(&state.game_file)
            .await
            .map_err(|error| format!("替换旧游戏数据失败：{error}"))?;
    }
    tokio::fs::rename(&tmp, &state.game_file)
        .await
        .map_err(|error| format!("替换游戏数据失败：{error}"))?;
    *state.game.write().await = data.clone();
    Ok(())
}

pub async fn update_game_data(state: &Arc<ServerState>) -> Result<Value, String> {
    state.crawling.store(true, Ordering::SeqCst);
    let result = update_game_data_inner(state).await;
    state.crawling.store(false, Ordering::SeqCst);
    result
}

/// 优先增量：版本没变就只补缺失实体；版本变了或缺缓存则全量。
async fn update_game_data_inner(state: &Arc<ServerState>) -> Result<Value, String> {
    let lang = state.lang().await;
    let (version, mut entries) = fetch_game_lists(state).await?;
    let needed = entity_paths(&entries);
    let cache = state.game.read().await.clone();

    let mut mode = "full";
    let mut jobs = needed.clone();

    if !cache.entries.is_empty() && cache.version == version {
        mode = "incremental";
        let missing: Vec<String> = needed
            .iter()
            .filter(|path| !cache.entries.contains_key(path.as_str()))
            .cloned()
            .collect();
        // 先沿用已有的实体条目，只补缺口。
        for (key, value) in cache.entries.iter() {
            if is_entity_path(key) {
                entries.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        jobs = missing;
        state
            .set_progress(format!(
                "{}（{} / {}）…",
                tr!(
                    lang,
                    "正在增量拉取游戏数据",
                    "正在增量拉取遊戲資料",
                    "不足分のゲームデータを取得中",
                    "누락된 게임 데이터 가져오는 중",
                    "Fetching missing game data"
                ),
                jobs.len(),
                needed.len()
            ))
            .await;
    } else if cache.entries.is_empty() {
        state
            .set_progress(format!(
                "{}（{}）…",
                tr!(
                    lang,
                    "正在全量拉取游戏数据",
                    "正在全量拉取遊戲資料",
                    "ゲームデータを全件取得中",
                    "게임 데이터 전체 가져오는 중",
                    "Fetching all game data"
                ),
                jobs.len()
            ))
            .await;
    } else {
        // 版本变化：实体数据可能整体变了，增量不足以保证一致 → 全量。
        state
            .set_progress(format!(
                "{} {} → {}（{}）…",
                tr!(
                    lang,
                    "版本",
                    "版本",
                    "バージョン",
                    "버전",
                    "Version"
                ),
                cache.version,
                version,
                jobs.len()
            ))
            .await;
    }

    let label = if mode == "incremental" {
        tr!(
            lang,
            "正在拉取缺失游戏数据",
            "正在補齊遊戲資料",
            "不足分を取得中",
            "누락분 가져오는 중",
            "Fetching missing game data"
        )
    } else {
        tr!(
            lang,
            "正在拉取游戏数据",
            "正在拉取遊戲資料",
            "ゲームデータを取得中",
            "게임 데이터 가져오는 중",
            "Fetching game data"
        )
    };
    fetch_entity_paths(state, &jobs, &mut entries, label).await;

    let data = GameData {
        version: version.clone(),
        fetched_at: stamp_now(),
        entries,
    };
    save_game_data(state, &data).await?;
    state
        .set_progress(format!(
            "{} v{version} · {} {} / {}",
            tr!(
                lang,
                "游戏数据已更新到",
                "遊戲資料已更新到",
                "ゲームデータを更新しました",
                "게임 데이터 업데이트 완료",
                "Game data updated to"
            ),
            tr!(
                lang,
                if mode == "incremental" { "增量" } else { "全量" },
                if mode == "incremental" { "增量" } else { "全量" },
                if mode == "incremental" { "差分" } else { "全件" },
                if mode == "incremental" { "증분" } else { "전체" },
                if mode == "incremental" { "incremental" } else { "full" }
            ),
            jobs.len(),
            data.entries.len()
        ))
        .await;
    state.log(
        "game",
        &format!(
            "游戏数据更新完成：mode={mode} version={version} fetched={} entries={}",
            jobs.len(),
            data.entries.len()
        ),
    );
    Ok(json!({
        "mode": mode,
        "version": version,
        "entries": data.entries.len(),
        "fetched": jobs.len(),
        "fetchedAt": data.fetched_at,
    }))
}

pub async fn check_toolbox_update(state: &Arc<ServerState>) -> Result<Value, String> {
    let lang = state.lang().await;
    state
        .set_progress(tr!(
            lang,
            "正在读取 Release 清单…",
            "正在讀取 Release 清單…",
            "Release マニフェストを読み込み中…",
            "Release 매니페스트 읽는 중…",
            "Reading the Release manifest…"
        ))
        .await;
    let bytes = fetch_manifest(state, false).await?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("GitHub manifest 不是 JSON：{error}"))?;
    let remote = manifest["commit"].as_str().unwrap_or("").to_string();
    let version = manifest["version"].as_str().unwrap_or("").to_string();
    if remote.is_empty() {
        return Err("GitHub manifest 缺少 commit".into());
    }
    let local = state.commit.read().await.clone();
    state
        .set_progress(if local == remote {
            format!(
                "{} · {local}",
                tr!(
                    lang,
                    "工具箱已是最新",
                    "工具箱已是最新",
                    "ツールボックスは最新です",
                    "툴박스가 최신입니다",
                    "Toolbox is up to date"
                )
            )
        } else {
            format!(
                "{} · {remote}",
                tr!(
                    lang,
                    "发现工具箱新版本",
                    "發現工具箱新版本",
                    "ツールボックスの新バージョン",
                    "툴박스 새 버전 발견",
                    "New toolbox version"
                )
            )
        })
        .await;
    Ok(json!({
        "available": local != remote,
        "local": local,
        "remote": remote,
        "version": version,
    }))
}

/// 清单也走镜像：检查版本期间同样可以「切换镜像」。
/// `quiet = true` 时不写启动页进度（启动时的自动更新不想在界面上刷一堆文字）。
async fn fetch_manifest(state: &Arc<ServerState>, quiet: bool) -> Result<Bytes, String> {
    let lang = state.lang().await;
    let mut failures: Vec<String> = Vec::new();
    let mut switches = 0usize;

    'rounds: loop {
        let generation = state.mirror_gen.load(Ordering::SeqCst);
        let start = state.mirror_index.load(Ordering::SeqCst);
        for (index, prefix) in rotated_bases(start) {
            let label = mirror_label(lang, index);
            let url = format!("{prefix}{DIST_MANIFEST_URL}");
            if !quiet {
                state
                    .set_progress(format!(
                        "{}（{label}）…",
                        tr!(
                            lang,
                            "正在读取 Release 清单",
                            "正在讀取 Release 清單",
                            "Release マニフェストを読み込み中",
                            "Release 매니페스트 읽는 중",
                            "Reading the Release manifest"
                        )
                    ))
                    .await;
            }
            match read_manifest_source(state, &url, generation).await {
                Ok(bytes) => {
                    state.log("update", &format!("{label} 读取清单成功"));
                    return Ok(bytes);
                }
                Err(error) if error == MIRROR_SWITCHED => {
                    switches += 1;
                    if switches > 8 {
                        return Err("镜像切换过于频繁，已停止检查".into());
                    }
                    state
                        .set_progress(tr!(
                            lang,
                            "已切换镜像，正在重新连接…",
                            "已切換鏡像，正在重新連線…",
                            "ミラーを切り替えました。再接続中…",
                            "미러를 전환했습니다. 다시 연결 중…",
                            "Mirror switched, reconnecting…"
                        ))
                        .await;
                    continue 'rounds;
                }
                Err(error) => {
                    state
                        .log("update", &format!("{label} 读取清单失败：{error}"));
                    let item = format!("{label}：{error}");
                    if !failures.contains(&item) {
                        failures.push(item);
                    }
                }
            }
        }
        return Err(format!(
            "读取清单失败（已尝试直连和 {} 个镜像）：{}",
            DIST_ARCHIVE_MIRRORS.len(),
            failures.join("；")
        ));
    }
}

async fn read_manifest_source(
    state: &Arc<ServerState>,
    url: &str,
    generation: usize,
) -> Result<Bytes, String> {
    if state.mirror_gen.load(Ordering::SeqCst) != generation {
        return Err(MIRROR_SWITCHED.into());
    }
    let response = state
        .client
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| format!("连接失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut data = Vec::new();
    loop {
        let chunk = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => chunk.map_err(|error| format!("传输中断：{error}"))?,
            Ok(None) => break,
            Err(_) => return Err("读取清单超时（30 秒没有收到数据）".into()),
        };
        if state.mirror_gen.load(Ordering::SeqCst) != generation {
            return Err(MIRROR_SWITCHED.into());
        }
        data.extend_from_slice(&chunk);
        if data.len() > 1024 * 1024 {
            return Err("manifest 体积异常".into());
        }
    }
    if data.is_empty() {
        return Err("响应为空".into());
    }
    Ok(Bytes::from(data))
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

async fn download_archive_source(
    state: &Arc<ServerState>,
    label: &str,
    url: &str,
    generation: usize,
) -> Result<Bytes, String> {
    let lang = state.lang().await;
    if state.mirror_gen.load(Ordering::SeqCst) != generation {
        return Err(MIRROR_SWITCHED.into());
    }
    let response = state
        .client
        .get(url)
        .timeout(Duration::from_secs(180))
        .header("accept", "application/octet-stream")
        .send()
        .await
        .map_err(|error| format!("连接失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let total = response.content_length();
    let mut downloaded = 0u64;
    let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = response.bytes_stream();
    loop {
        // 只给「多久没收到数据」设上限：整体超时会让慢速但活着的下载被误杀，
        // 而没有上限时（直连 GitHub 资产经常挂在半路）会一直卡住，自动更新永远完不成。
        let chunk = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => chunk.map_err(|error| format!("传输中断：{error}"))?,
            Ok(None) => break,
            Err(_) => {
                return Err(format!(
                    "{} 秒没有收到数据，已放弃",
                    STREAM_IDLE_TIMEOUT.as_secs()
                ))
            }
        };
        if state.mirror_gen.load(Ordering::SeqCst) != generation {
            return Err(MIRROR_SWITCHED.into());
        }
        downloaded += chunk.len() as u64;
        data.extend_from_slice(&chunk);
        let prefix = format!(
            "{}（{label}）",
            tr!(
                lang,
                "正在下载静态包",
                "正在下載靜態包",
                "静的パッケージをダウンロード中",
                "정적 패키지 다운로드 중",
                "Downloading the package"
            )
        );
        let text = match total {
            Some(total) if total > 0 => format!(
                "{prefix} {} / {} · {:.0}%",
                human_size(downloaded),
                human_size(total),
                downloaded as f64 * 100.0 / total as f64
            ),
            _ => format!("{prefix} {}…", human_size(downloaded)),
        };
        state.set_progress(text).await;
    }
    if data.is_empty() {
        return Err("下载内容为空".into());
    }
    Ok(Bytes::from(data))
}

async fn download_toolbox_archive(state: &Arc<ServerState>) -> Result<Bytes, String> {
    let lang = state.lang().await;
    let mut failures: Vec<String> = Vec::new();
    let mut switches = 0usize;

    'rounds: loop {
        let generation = state.mirror_gen.load(Ordering::SeqCst);
        let start = state.mirror_index.load(Ordering::SeqCst);
        let sources = rotated_sources(start);
        for (step, (index, url)) in sources.iter().enumerate() {
            let label = mirror_label(lang, *index);
            state
                .set_progress(format!(
                    "{}（{label}）…",
                    tr!(
                        lang,
                        "正在下载静态包",
                        "正在下載靜態包",
                        "静的パッケージをダウンロード中",
                        "정적 패키지 다운로드 중",
                        "Downloading the package"
                    )
                ))
                .await;
            state.log("update", &format!("尝试 {label} 下载：{url}"));
            match download_archive_source(state, &label, url, generation).await {
                Ok(bytes) => {
                    state.log(
                        "update",
                        &format!("{label} 下载成功：{}", human_size(bytes.len() as u64)),
                    );
                    return Ok(bytes);
                }
                Err(error) if error == MIRROR_SWITCHED => {
                    // 用户手动切换镜像：丢掉当前连接，用新镜像重来。
                    switches += 1;
                    if switches > 8 {
                        return Err("镜像切换过于频繁，已停止下载".into());
                    }
                    state
                        .set_progress(tr!(
                            lang,
                            "已切换镜像，正在重新连接…",
                            "已切換鏡像，正在重新連線…",
                            "ミラーを切り替えました。再接続中…",
                            "미러를 전환했습니다. 다시 연결 중…",
                            "Mirror switched, reconnecting…"
                        ))
                        .await;
                    continue 'rounds;
                }
                Err(error) => {
                    state.log("update", &format!("{label} 下载失败：{error}"));
                    if !failures.iter().any(|item| item == &format!("{label}：{error}")) {
                        failures.push(format!("{label}：{error}"));
                    }
                    if step + 1 < sources.len() {
                        state
                            .set_progress(format!(
                                "{}（{label}）…",
                                tr!(
                                    lang,
                                    "下载失败，正在切换镜像",
                                    "下載失敗，正在切換鏡像",
                                    "ダウンロード失敗。ミラーを切り替え中",
                                    "다운로드 실패. 미러 전환 중",
                                    "Download failed, switching mirror"
                                )
                            ))
                            .await;
                    }
                }
            }
        }
        return Err(format!(
            "静态包下载失败（已尝试直连和 {} 个镜像）：{}",
            DIST_ARCHIVE_MIRRORS.len(),
            failures.join("；")
        ));
    }
}

/// 把 tar 条目路径规整成安全相对路径：丢掉前导 `.`，拒绝 `..` 与绝对路径。
/// GitHub 发布的包是 `./index.html`，本地 build.rs 打的包是 `index.html`，两种都要吃下。
fn safe_relative_path(path: &std::path::Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::ParentDir | std::path::Component::RootDir => return None,
            other => relative.push(other.as_os_str()),
        }
    }
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

/// 解开 toolbox.tar.gz 到目标目录（拒绝 `..` 与符号链接），返回文件数。
/// 升级流程与「解出编译进 exe 的内置包」共用。
pub fn extract_static_archive(bytes: &[u8], target: &std::path::Path) -> Result<usize, String> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| format!("读取静态包失败：{error}"))?;
    std::fs::create_dir_all(target).map_err(|error| format!("创建静态目录失败：{error}"))?;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("读取静态条目失败：{error}"))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_dir() {
            // 目录不单独落盘：解文件时按需 create_dir_all
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| format!("读取静态路径失败：{error}"))?
            .to_path_buf();
        let Some(relative) = safe_relative_path(&path) else {
            continue;
        };
        if let Some(parent) = target.join(&relative).parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("创建解压目录失败：{error}"))?;
        }
        entry
            .unpack(target.join(relative))
            .map_err(|error| format!("解压静态文件失败：{error}"))?;
        count += 1;
    }
    Ok(count)
}

/// 启动时的自动更新：有新版本就后台下载并替换静态包，用户不用点「检查更新」。
///
/// 返回 `Ok(Some(commit))` = 真的更新到了这个 commit；`Ok(None)` = 已是最新；
/// `Err(_)` = 网络 / 解压失败（静默跳过，启动页仍可手动更新）。
pub async fn auto_update_toolbox(state: &Arc<ServerState>) -> Result<Option<String>, String> {
    let bytes = fetch_manifest(state, true).await?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("GitHub manifest 不是 JSON：{error}"))?;
    let remote = manifest["commit"].as_str().unwrap_or("").to_string();
    if remote.is_empty() {
        return Err("GitHub manifest 缺少 commit".into());
    }
    let local = state.commit.read().await.clone();
    if local == remote {
        state
            .log("update", &format!("启动时检查工具箱版本：已是最新 · {local}"));
        return Ok(None);
    }
    let lang = state.lang().await;
    state
        .set_progress(format!(
            "{} {remote}…",
            tr!(
                lang,
                "发现工具箱新版本，正在自动更新",
                "發現工具箱新版本，正在自動更新",
                "ツールボックスの新バージョンを検出。自動更新中",
                "툴박스 새 버전 발견. 자동 업데이트 중",
                "New toolbox version found, auto-updating"
            )
        ))
        .await;
    let result = update_toolbox(state).await?;
    let commit = result["commit"].as_str().unwrap_or(&remote).to_string();
    state.log(
        "update",
        &format!("启动时自动更新工具箱：{local} → {commit}"),
    );
    Ok(Some(commit))
}

pub async fn update_toolbox(state: &Arc<ServerState>) -> Result<Value, String> {
    // 自动更新和手动「更新」互斥：同时替换同一个目录会把静态文件搅烂。
    if state
        .updating
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("工具箱正在更新中，请稍候".into());
    }
    let result = update_toolbox_inner(state).await;
    state.updating.store(false, Ordering::SeqCst);
    result
}

async fn update_toolbox_inner(state: &Arc<ServerState>) -> Result<Value, String> {
    let lang = state.lang().await;
    state
        .set_progress(tr!(
            lang,
            "正在获取工具箱更新…",
            "正在取得工具箱更新…",
            "ツールボックスの更新を取得中…",
            "툴박스 업데이트 받는 중…",
            "Fetching the toolbox update…"
        ))
        .await;
    let bytes = download_toolbox_archive(state).await?;
    state
        .set_progress(tr!(
            lang,
            "静态包下载完成，正在解压…",
            "靜態包下載完成，正在解壓…",
            "パッケージの取得完了。展開中…",
            "패키지 다운로드 완료. 압축 해제 중…",
            "Package downloaded, extracting…"
        ))
        .await;

    let temp = state.static_dir.with_extension("new");
    let backup = state.static_dir.with_extension("bak");
    let _ = std::fs::remove_dir_all(&temp);
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::create_dir_all(&temp).map_err(|error| format!("创建临时目录失败：{error}"))?;

    let count = {
        state
            .set_progress(tr!(
                lang,
                "正在解压静态包…",
                "正在解壓靜態包…",
                "パッケージを展開中…",
                "패키지 압축 해제 중…",
                "Extracting the package…"
            ))
            .await;
        extract_static_archive(bytes.as_ref(), &temp)?
    };

    if !temp.join("index.html").is_file() {
        return Err("静态包缺少 index.html".into());
    }
    let manifest = std::fs::read_to_string(temp.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let commit = manifest["commit"].as_str().unwrap_or("").to_string();
    let build = manifest["builtAt"].as_str().unwrap_or("").to_string();

    state
        .set_progress(tr!(
            lang,
            "正在替换静态文件…",
            "正在替換靜態檔案…",
            "静的ファイルを置き換え中…",
            "정적 파일 교체 중…",
            "Replacing static files…"
        ))
        .await;
    std::fs::rename(&state.static_dir, &backup)
        .map_err(|error| format!("备份旧静态目录失败：{error}"))?;
    if let Err(error) = std::fs::rename(&temp, &state.static_dir) {
        let _ = std::fs::rename(&backup, &state.static_dir);
        return Err(format!("替换静态目录失败：{error}"));
    }
    let _ = std::fs::remove_dir_all(&backup);
    *state.commit.write().await = commit.clone();
    *state.build.write().await = build.clone();
    state
        .set_progress(format!(
            "{} {}",
            tr!(
                lang,
                "工具箱已更新到",
                "工具箱已更新到",
                "ツールボックスを更新しました",
                "툴박스 업데이트 완료",
                "Toolbox updated to"
            ),
            if commit.is_empty() {
                tr!(
                    lang,
                    "最新版本",
                    "最新版本",
                    "最新版",
                    "최신 버전",
                    "the latest version"
                )
            } else {
                &commit
            }
        ))
        .await;
    state.log(
        "update",
        &format!("工具箱静态包更新完成：commit={commit} build={build} files={count}"),
    );
    Ok(json!({
        "commit": commit,
        "build": build,
    }))
}

pub async fn start(static_dir: PathBuf, data_dir: PathBuf) -> Result<Arc<ServerState>, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;

    let manifest = std::fs::read_to_string(static_dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    let commit = manifest["commit"].as_str().unwrap_or("").to_string();
    let build = manifest["builtAt"].as_str().unwrap_or("").to_string();
    let game_file = data_dir.join("game-data.json");
    let models_file = data_dir.join("free-models.json");
    let key_file = data_dir.join("opencode-key.json");
    let icons_dir = data_dir.join("icons");
    let settings_file = data_dir.join("settings.json");
    let settings = read_settings(&settings_file);
    let language = Lang::from_tag(&settings.language);
    let theme = if settings.theme == "dark" { "dark" } else { "light" };
    let plugins = Plugins::new(&data_dir);
    let (frames, _) = tokio::sync::broadcast::channel::<Value>(512);
    let api_key = std::fs::read_to_string(&key_file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value["key"].as_str().map(str::to_string))
        .unwrap_or_default();
    let game = std::fs::read_to_string(&game_file)
        .ok()
        .and_then(|text| serde_json::from_str::<GameData>(&text).ok())
        .unwrap_or_default();

    let state = Arc::new(ServerState {
        static_dir: static_dir.clone(),
        data_dir,
        game_file,
        icons_dir,
        game: RwLock::new(game),
        client,
        models: RwLock::new(Vec::new()),
        models_at: RwLock::new(None),
        models_file,
        api_key: RwLock::new(api_key),
        key_file,
        mirror_index: AtomicUsize::new(0),
        mirror_gen: AtomicUsize::new(0),
        theme: RwLock::new(theme.to_string()),
        commit: RwLock::new(commit),
        build: RwLock::new(build),
        progress: RwLock::new(String::new()),
        url: format!("http://127.0.0.1:{TOOLBOX_PORT}"),
        ready: AtomicBool::new(false),
        error: RwLock::new(String::new()),
        ws: WsHub::new(),
        app: RwLock::new(None),
        zoom: AtomicU64::new(1.0f64.to_bits()),
        crawling: AtomicBool::new(false),
        workshop_target: RwLock::new(None),
        updating: AtomicBool::new(false),
        plugins,
        lang: RwLock::new(language),
        frames: frames.clone(),
        page_session: RwLock::new(None),
        settings_file,
    });
    let serve_dir =
        ServeDir::new(&static_dir).not_found_service(ServeFile::new(static_dir.join("200.html")));
    let app = Router::new()
        .route("/v1/models", get(models_handler))
        .route("/v1/chat/completions", post(chat_handler))
        .route("/api/{*path}", any(api_proxy))
        // `/icons/*` 同时承载「本地化的游戏图标」和「工具箱自带的静态图标」，
        // 必须走同一个处理器：直接 nest 游戏图标目录会把工具箱的 icons/ 整个遮蔽。
        .route("/icons/{*path}", get(icons_handler))
        // 工具箱的 PWA Service Worker 会把旧版本页面缓存住（见 SW_KILL）。
        .route("/sw.js", get(sw_handler))
        .route("/registerSW.js", get(sw_handler))
        .fallback_service(serve_dir)
        .layer(axum::middleware::from_fn(cache_control))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(state.clone());

    let ws_app = Router::new()
        .route("/", get(ws_handler))
        .with_state(state.clone());

    let http_listener = TcpListener::bind(("127.0.0.1", TOOLBOX_PORT))
        .await
        .map_err(|error| format!("无法监听工具箱端口 {TOOLBOX_PORT}: {error}"))?;
    let ws_listener = TcpListener::bind(("127.0.0.1", WS_PORT))
        .await
        .map_err(|error| format!("无法监听 WS 端口 {WS_PORT}: {error}"))?;
    // 工坊反代端口：被占用时只记日志，不影响启动器主功能
    let workshop_listener = match TcpListener::bind(("127.0.0.1", WORKSHOP_PORT)).await {
        Ok(listener) => Some(listener),
        Err(error) => {
            state.log(
                "workshop",
                &format!("无法监听工坊反代端口 {WORKSHOP_PORT}：{error}"),
            );
            None
        }
    };

    state.ready.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let _ = axum::serve(http_listener, app).await;
    });
    tokio::spawn(async move {
        let _ = axum::serve(ws_listener, ws_app).await;
    });
    if let Some(listener) = workshop_listener {
        let workshop_app = Router::new()
            .fallback(workshop_proxy)
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, workshop_app).await;
        });
    }
    // 插件：桥接宿主能力后启动所有启用中的插件，再把 WS 帧泵起来。
    {
        let bridge = Arc::new(ServerBridge {
            state: Arc::downgrade(&state),
        });
        state.plugins.attach_bridge(bridge);
        state.plugins.start_all();
    }
    {
        let frame_state = state.clone();
        let mut frame_rx = frames.subscribe();
        tokio::spawn(async move {
            loop {
                let Ok(record) = frame_rx.recv().await else {
                    break;
                };
                let outgoing = record["direction"].as_str() == Some("send");
                let frame = record["frame"].clone();
                frame_state.plugins.debug_frame(frame.clone(), outgoing);
            }
        });
    }

    let init_state = state.clone();
    tokio::spawn(async move {
        // 命中磁盘缓存时不会请求上游：一天只拉一次。
        let ids = init_state.model_ids(false).await;
        if !ids.is_empty() {
            // 模型列表就绪：让插件（免费模型）重新同步一次，
            // 否则第一次 hello 常常赶在列表拉回来之前，只能空跑。
            init_state
                .plugins
                .event(json!({ "type": "resync", "models": ids }));
        }
    });
    tokio::spawn(refresh_models(state.clone()));

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ids_hide_free_suffix_and_drop_big_pickle() {
        assert_eq!(public_model_id("beam-free").as_deref(), Some("beam"));
        assert_eq!(
            public_model_id("deepseek-v4.1-flash-free").as_deref(),
            Some("deepseek-v4.1-flash")
        );
        assert_eq!(public_model_id("big-pickle"), None);
        assert_eq!(public_model_id("test"), None);
        assert_eq!(public_model_id("-free"), None);
        assert_eq!(upstream_model_id("beam"), "beam-free");
    }

    #[test]
    fn extract_public_models_filters_sorts_and_dedups() {
        let payload = json!({
            "data": [
                { "id": "big-pickle" },
                { "id": "beam-free" },
                { "id": "jev-1.13-free" },
                { "id": "glm-5.3-flash-free" },
                { "id": "claude-opus-5" },
                { "id": "beam-free" }
            ]
        });
        assert_eq!(
            extract_public_models(&payload),
            vec!["beam".to_string(), "glm-5.3-flash".to_string()]
        );
    }

    #[test]
    fn api_key_format_rules() {
        assert!(!looks_like_api_key(""));
        assert!(!looks_like_api_key("public"));
        assert!(!looks_like_api_key("undefined"));
        assert!(!looks_like_api_key("short"));
        assert!(!looks_like_api_key("has space here"));
        assert!(looks_like_api_key("sk-abcdef123456"));
        assert!(looks_like_api_key("oc_live.9f3a~Z+/="));
    }

    #[test]
    fn bearer_prefix_is_stripped() {
        assert_eq!(strip_bearer("Bearer sk-abcdef123456"), "sk-abcdef123456");
        assert_eq!(strip_bearer("bearer sk-abcdef123456"), "sk-abcdef123456");
        assert_eq!(strip_bearer("  sk-abcdef123456  "), "sk-abcdef123456");
        assert_eq!(strip_bearer("Bearer public"), "public");
        assert_eq!(strip_bearer(""), "");
    }

    #[test]
    fn models_cache_roundtrip() {
        let path = std::env::temp_dir().join("coconut-models-test.json");
        let ids = vec!["beam".to_string(), "glm-5.3-flash".to_string()];
        write_models_cache(&path, &ids);
        let (cached, stamp) = read_models_cache(&path).expect("cache should round-trip");
        assert_eq!(cached, ids);
        let age = SystemTime::now().duration_since(stamp).unwrap_or_default();
        assert!(age.as_secs() < 5);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mirror_rotation_wraps_to_first_source() {
        let count = 1 + DIST_ARCHIVE_MIRRORS.len();
        // 镜像标签按序号 + 当前语言生成
        assert_eq!(mirror_label(Lang::ZhCn, 0), "直连");
        assert_eq!(mirror_label(Lang::ZhCn, 1), "镜像 1");
        assert_eq!(mirror_label(Lang::En, 0), "Direct");
        assert_eq!(mirror_label(Lang::En, 2), "Mirror 2");
        assert_eq!(mirror_label(Lang::Ja, 1), "ミラー 1");

        // 从「最后一个镜像」再切换一次，回到最开始的直连地址。
        let rotated = rotated_sources(count - 1);
        assert_eq!(rotated[0].0, count - 1);
        assert_eq!(rotated[1].0, 0);
        assert_eq!(rotated[1].1, DIST_ARCHIVE_URL);

        // 越界索引安全取模。
        assert_eq!(rotated_sources(count).first().map(|item| item.0), Some(0));
    }

    #[test]
    fn entity_paths_expand_character_lists() {
        let mut entries = HashMap::new();
        entries.insert(
            "/api/v1/list/character".to_string(),
            json!([{ "name": "今汐" }]),
        );
        let paths = entity_paths(&entries);
        assert_eq!(paths.len(), 4);
        assert!(paths.iter().any(|path| path.starts_with("/api/v1/info/character/")));
        assert!(paths.iter().any(|path| path.starts_with("/api/v2/info/character/")));
        assert!(paths.iter().any(|path| path.starts_with("/api/v3/info/character/")));
        assert!(paths.iter().any(|path| path.starts_with("/api/v1/recommend-weapon/")));
    }

    #[test]
    fn icon_hash_is_stable() {
        assert_eq!(hash_url("https://static/a.webp"), hash_url("https://static/a.webp"));
        assert_ne!(hash_url("https://static/a.webp"), hash_url("https://static/b.webp"));
        assert_eq!(hash_url("x").len(), 16);
    }


    #[test]
    fn archive_paths_handle_both_layouts() {
        use std::path::Path;
        assert_eq!(
            safe_relative_path(Path::new("./index.html")),
            Some(PathBuf::from("index.html"))
        );
        assert_eq!(
            safe_relative_path(Path::new("index.html")),
            Some(PathBuf::from("index.html"))
        );
        assert_eq!(
            safe_relative_path(Path::new("./_app/immutable/a.js")),
            Some(PathBuf::from("_app/immutable/a.js"))
        );
        assert_eq!(safe_relative_path(Path::new("../escape.txt")), None);
        assert_eq!(safe_relative_path(Path::new("/abs.txt")), None);
        assert_eq!(safe_relative_path(Path::new("./")), None);
    }

    #[test]
    fn static_archive_roundtrip() {
        // 造一个和 build.rs 产出一致的包：条目不带 "./" 前缀
        let root = std::env::temp_dir().join("coconut-archive-test");
        let source = root.join("source");
        let target = root.join("target");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(source.join("_app")).expect("mkdir");
        std::fs::write(source.join("index.html"), b"<html></html>").expect("write html");
        std::fs::write(source.join("_app/app.js"), b"console.log(1)").expect("write js");
        std::fs::write(source.join("manifest.json"), b"{\"commit\":\"abc\"}").expect("write manifest");

        let mut archive = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut archive, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            builder.append_dir_all(".", &source).expect("tar");
            let encoder = builder.into_inner().expect("tar finish");
            encoder.finish().expect("gz finish");
        }

        let count = extract_static_archive(&archive, &target).expect("extract");
        assert_eq!(count, 3);
        assert!(target.join("index.html").is_file());
        assert!(target.join("_app/app.js").is_file());
        assert_eq!(
            std::fs::read_to_string(target.join("manifest.json")).expect("read"),
            "{\"commit\":\"abc\"}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }




    #[test]
    fn static_cache_policy_keeps_html_fresh() {
        // 页面入口必须回源校验，否则更新后 WebView2 还是旧页面
        assert_eq!(static_cache_value("/"), Some("no-cache"));
        assert_eq!(static_cache_value("/200.html"), Some("no-cache"));
        assert_eq!(static_cache_value("/manifest.json"), Some("no-cache"));
        assert_eq!(static_cache_value("/sw.js"), Some("no-cache"));
        // 带内容哈希的资源可以长缓存
        assert_eq!(
            static_cache_value("/_app/immutable/chunks/BDPPuBhx.js"),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(
            static_cache_value("/icons/abc123.webp"),
            Some("public, max-age=31536000, immutable")
        );
        // 代理接口不动缓存头
        assert_eq!(static_cache_value("/api/v1/characters"), None);
        assert_eq!(static_cache_value("/v1/models"), None);
    }

    #[test]
    fn service_worker_shim_drops_workbox_precache() {
        // 空壳 SW 不能注册 fetch 处理器（否则还会拦请求、继续喂缓存）
        assert!(!SW_KILL.contains("addEventListener('fetch'"));
        assert!(SW_KILL.contains("caches.delete"));
        assert!(SW_KILL.contains("self.skipWaiting()"));
        // 只清 Workbox 预缓存，业务自己的图片缓存（data-cdn）要留着
        assert!(SW_KILL.contains("/workbox|precache/i"));
        assert!(!SW_KILL.contains("data-cdn"));
    }

    #[test]
    fn workshop_cookie_and_location_rewrite() {
        // 去掉 Domain，其余属性原样保留
        assert_eq!(
            rewrite_set_cookie("sid=abc; Domain=.wuwa-afyg-share.200503.xyz; Path=/; HttpOnly; Secure; SameSite=Lax"),
            "sid=abc; Path=/; HttpOnly; Secure; SameSite=Lax"
        );
        assert_eq!(
            rewrite_set_cookie("a=1; Path=/; SameSite=None; Secure"),
            "a=1; Path=/; SameSite=None; Secure"
        );
        // Location：上游域换成本机反代
        assert_eq!(
            rewrite_location(
                "https://wuwa-afyg-share.200503.xyz/login?next=%2F",
                "https://wuwa-afyg-share.200503.xyz"
            ),
            "http://127.0.0.1:47822/login?next=%2F"
        );
        // 不是上游域就不动
        assert_eq!(
            rewrite_location("/buff-sets", "https://wuwa-afyg-share.200503.xyz"),
            "/buff-sets"
        );
    }

    #[test]
    fn icon_resolution_prefers_local_then_static() {
        let root = std::env::temp_dir().join("coconut-icons-test");
        let icons = root.join("icons");
        let static_dir = root.join("static");
        let static_icons = static_dir.join("icons");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&static_icons).expect("mkdir static");
        std::fs::create_dir_all(&icons).expect("mkdir icons");
        std::fs::write(static_icons.join("placeholder.svg"), b"<svg/>").expect("write static");
        std::fs::write(icons.join("abc123.webp"), b"webp").expect("write local");

        // 本地游戏图标
        let local = resolve_icon(&icons, &static_dir, "abc123.webp").expect("local icon");
        assert_eq!(local, icons.join("abc123.webp"));
        // 工具箱自带图标（同一路径前缀，不能被遮蔽）
        let bundled = resolve_icon(&icons, &static_dir, "placeholder.svg").expect("bundled icon");
        assert_eq!(bundled, static_icons.join("placeholder.svg"));
        // 目录穿越与不存在的文件
        assert!(resolve_icon(&icons, &static_dir, "../secret.txt").is_none());
        assert!(resolve_icon(&icons, &static_dir, "missing.webp").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn icons_route_serves_local_and_static_icons() {
        use axum::http::Request;
        use tower::ServiceExt;

        let root = std::env::temp_dir().join("coconut-icons-route-test");
        let icons = root.join("icons");
        let static_dir = root.join("static");
        let static_icons = static_dir.join("icons").join("weapon-type");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&static_icons).expect("mkdir static icons");
        std::fs::create_dir_all(&icons).expect("mkdir local icons");
        std::fs::write(static_icons.join("长刃.webp"), b"bundled").expect("write bundled");
        std::fs::write(icons.join("abc123.webp"), b"local").expect("write local");

        let state = Arc::new(ServerState {
            static_dir: static_dir.clone(),
            data_dir: root.clone(),
            game_file: root.join("game-data.json"),
            icons_dir: icons.clone(),
            game: RwLock::new(GameData::default()),
            client: reqwest::Client::new(),
            models: RwLock::new(Vec::new()),
            models_at: RwLock::new(None),
            models_file: root.join("free-models.json"),
            api_key: RwLock::new(String::new()),
            key_file: root.join("opencode-key.json"),
            mirror_index: AtomicUsize::new(0),
            mirror_gen: AtomicUsize::new(0),
            theme: RwLock::new("light".into()),
            commit: RwLock::new(String::new()),
            build: RwLock::new(String::new()),
            progress: RwLock::new(String::new()),
            url: String::new(),
            ready: AtomicBool::new(true),
            error: RwLock::new(String::new()),
            ws: WsHub::new(),
            app: RwLock::new(None),
            zoom: AtomicU64::new(1.0f64.to_bits()),
            crawling: AtomicBool::new(false),
            workshop_target: RwLock::new(None),
            updating: AtomicBool::new(false),
            plugins: Plugins::new(&root),
            lang: RwLock::new(Lang::En),
            frames: tokio::sync::broadcast::channel(8).0,
            page_session: RwLock::new(None),
            settings_file: root.join("settings.json"),
        });
        let app = Router::new()
            .route("/icons/{*path}", get(icons_handler))
            .with_state(state);

        let fetch = |uri: &'static str| {
            let app = app.clone();
            async move {
                let response = app
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .expect("router");
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                    .await
                    .expect("body");
                (status, body)
            }
        };

        let (status, body) = fetch("/icons/abc123.webp").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"local");

        // 工具箱自带图标不会被游戏图标目录遮蔽（含中文文件名）
        let (status, body) = fetch("/icons/weapon-type/%E9%95%BF%E5%88%83.webp").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"bundled");

        let (status, _) = fetch("/icons/missing.webp").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn transcode_reencodes_as_webp() {
        use image::ImageEncoder;
        let mut png = Vec::new();
        {
            let image = image::RgbaImage::from_pixel(8, 4, image::Rgba([10, 20, 30, 255]));
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(image.as_raw(), 8, 4, image::ExtendedColorType::Rgba8)
                .expect("png encode");
        }
        let webp = transcode_webp(&png).expect("transcode");
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let decoded = image::load_from_memory(&webp).expect("decode webp");
        assert_eq!((decoded.width(), decoded.height()), (8, 4));
        assert_eq!(decoded.to_rgba8().get_pixel(3, 2).0, [10, 20, 30, 255]);
    }
}
