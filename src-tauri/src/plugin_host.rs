//! 插件管理器：目录扫描、启用状态、worker 生命周期、日志、错误计数。
//!
//! 布局（`%APPDATA%\com.coconut.launcher\`）：
//! ```text
//! plugins/
//!   debug-terminal.js     内置插件，首次启动导出；之后以目录文件为准
//!   my-plugin.js          用户插件，拖进来即可
//!   state.json            每个插件的 enabled / errors / lastError
//!   storage/<id>.json     插件私有 KV
//! logs/plugins.log        插件日志（界面不提供查看入口，只看文件）
//! ```

use crate::plugin::{scan_plugins, Permission, Plugin, PluginInfo, PluginStates};
use crate::plugin_rt::{self, PluginBridge, Signal, Worker};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 内置插件的源码。启动时先复制到插件目录，之后**只从插件目录加载**：
/// 用户改过 / 删过都以目录里的文件为准；只有文件不存在时才重新导出。
pub const BUILTIN_PLUGINS: &[(&str, &str)] = &[(
    "debug-terminal",
    include_str!("../../builtin/debug-terminal.js"),
)];

/// 日志文件超过这个大小就滚成 `.1`（只留一份旧的，够排查即可）。
const LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 每次启动工具箱都会把旧日志备份一份，只留最近这么多份。
const LOG_BACKUPS: usize = 10;
/// 目录轮询间隔：拖进新文件后最多这么久自动加载。
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// 把缺失的内置插件写到插件目录；已存在的文件不覆盖。
fn materialize_builtins(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    for (id, source) in BUILTIN_PLUGINS {
        let path = dir.join(format!("{id}.js"));
        if path.exists() {
            continue;
        }
        if let Err(error) = std::fs::write(&path, source) {
            eprintln!("[plugin] 导出内置插件 {id} 失败：{error}");
        }
    }
}

fn is_builtin_id(id: &str) -> bool {
    BUILTIN_PLUGINS.iter().any(|(builtin, _)| *builtin == id)
}

/// 插件日志只写文件；`console.*` 走内置调试终端，不走这里。
#[derive(Clone)]
pub struct LogFile {
    path: PathBuf,
}

impl LogFile {
    pub fn new(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self { path }
    }

    pub fn line(&self, line: &str) {
        self.rotate();
        let entry = format!("{} {line}\n", utc_stamp());
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(entry.as_bytes()));
    }

    /// 每次启动工具箱：把旧日志改名成 `plugins-<时间>.log` 备份，再开一份新的。
    pub fn begin_session(&self, reason: &str) {
        let Some(dir) = self.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        let has_content = std::fs::metadata(&self.path)
            .map(|meta| meta.len() > 0)
            .unwrap_or(false);
        if has_content {
            let stamp = utc_stamp()
                .replace(' ', "_")
                .replace(':', "-")
                .replace('Z', "");
            let backup = dir.join(format!("plugins-{stamp}.log"));
            let _ = std::fs::rename(&self.path, &backup);
            prune_backups(&dir);
        }
        let header = format!("# ─── 工具箱启动：{reason} · {} ───\n", utc_stamp());
        let _ = std::fs::write(&self.path, header);
    }

    fn rotate(&self) {
        let oversized = std::fs::metadata(&self.path)
            .map(|meta| meta.len() > LOG_MAX_BYTES)
            .unwrap_or(false);
        if oversized {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
    }
}

/// 备份只留最近的 `LOG_BACKUPS` 份（文件名里带时间戳，按名字排序就是按时间排序）。
fn prune_backups(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with("plugins-") && name.ends_with(".log")
                })
                .unwrap_or(false)
        })
        .collect();
    if backups.len() <= LOG_BACKUPS {
        return;
    }
    backups.sort();
    let drop = backups.len() - LOG_BACKUPS;
    for path in backups.into_iter().take(drop) {
        let _ = std::fs::remove_file(path);
    }
}

/// 日志时间戳统一用 UTC（`2026-10-29 11:40:01Z`），不引时区依赖。
fn utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or_default();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的 days → (y, m, d)，避免为了时间戳引入 chrono。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub struct Plugins {
    dir: PathBuf,
    state_path: PathBuf,
    storage_dir: PathBuf,
    states: Arc<Mutex<PluginStates>>,
    catalog: Arc<Mutex<Vec<Plugin>>>,
    workers: Arc<Mutex<HashMap<String, Worker>>>,
    log: LogFile,
    bridge: Arc<Mutex<Option<Arc<dyn PluginBridge>>>>,
    /// 最近一次「工具箱页面就绪」的信息：轮询中途起的新插件要补一次 hello
    last_hello: Arc<Mutex<Option<Value>>>,
}

/// 处理一个插件的信号（成功清零、出错计数、连续 3 次自动停用）。
///
/// 抽成自由函数是因为目录轮询线程也要用它：新拖进来的插件必须在轮询线程里直接起起来。
fn pump_signals(
    id: String,
    rx: Receiver<Signal>,
    states: Arc<Mutex<PluginStates>>,
    workers: Arc<Mutex<HashMap<String, Worker>>>,
    state_path: PathBuf,
    log: LogFile,
) {
    std::thread::Builder::new()
        .name(format!("coconut-plugin-signals-{id}"))
        .spawn(move || {
            for signal in rx {
                match signal {
                    Signal::Ok => {
                        if let Ok(mut states) = states.lock() {
                            let state = states.get_mut(&id);
                            if state.errors != 0 {
                                state.errors = 0;
                                state.last_error.clear();
                            }
                        }
                    }
                    Signal::Error(message) => {
                        let mut disabled = false;
                        if let Ok(mut states) = states.lock() {
                            let state = states.get_mut(&id);
                            state.errors += 1;
                            state.last_error = message.clone();
                            if state.errors >= plugin_rt::MAX_CONSECUTIVE_ERRORS && state.enabled {
                                state.enabled = false;
                                disabled = true;
                            }
                            let _ = states.save(&state_path);
                        }
                        log.line(&format!("[plugin] {id} 出错：{message}"));
                        if disabled {
                            if let Ok(mut workers) = workers.lock() {
                                workers.remove(&id);
                            }
                            log.line(&format!(
                                "[plugin] {id} 连续 {} 次出错，已自动停用",
                                plugin_rt::MAX_CONSECUTIVE_ERRORS
                            ));
                            break;
                        }
                    }
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|error| eprintln!("[plugin] 信号线程启动失败：{error}"));
}

impl Plugins {
    pub fn new(data_dir: &Path) -> Self {
        let dir = data_dir.join("plugins");
        materialize_builtins(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let state_path = dir.join("state.json");
        let storage_dir = dir.join("storage");
        let _ = std::fs::create_dir_all(&storage_dir);
        let states = PluginStates::load(&state_path);
        let plugins = Self {
            dir,
            state_path,
            storage_dir,
            states: Arc::new(Mutex::new(states)),
            catalog: Arc::new(Mutex::new(Vec::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            log: LogFile::new(data_dir.join("logs").join("plugins.log")),
            bridge: Arc::new(Mutex::new(None)),
            last_hello: Arc::new(Mutex::new(None)),
        };
        plugins.refresh_catalog();
        plugins
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn log_line(&self, line: String) {
        self.log.line(&line);
    }

    /// 工具箱每次启动时调用：备份旧日志、开一份新的。
    pub fn begin_log_session(&self, reason: &str) {
        self.log.begin_session(reason);
        let total = self.list().len();
        let running = self
            .workers
            .lock()
            .map(|workers| workers.len())
            .unwrap_or(0);
        self.log
            .line(&format!("# 插件 {total} 个，正在运行 {running} 个"));
    }

    /// 供宿主（比如插件日志转发）追加一行日志。
    pub fn push_log(&self, line: String) {
        self.log_line(line);
    }

    /// 重新扫描插件目录。内置插件只负责“缺失时导出”，之后一律以目录里的文件为准。
    pub fn refresh_catalog(&self) {
        materialize_builtins(&self.dir);
        let mut catalog = scan_plugins(&self.dir);
        for plugin in &mut catalog {
            plugin.builtin = is_builtin_id(&plugin.id);
        }
        if let Ok(mut slot) = self.catalog.lock() {
            *slot = catalog;
        }
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        self.refresh_catalog();
        let catalog = self
            .catalog
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        let states = self
            .states
            .lock()
            .map(|states| states.clone())
            .unwrap_or_default();
        catalog
            .iter()
            .map(|plugin| PluginInfo::from_plugin(plugin, &states.state(&plugin.id)))
            .collect()
    }

    /// 清理插件目录里已经不存在的插件留下的 state.json 条目和 storage/<id>.json。
    /// 返回 `{ states, storage, workers }`（清理数量）。
    pub fn cleanup_dead(&self) -> Result<serde_json::Value, String> {
        self.refresh_catalog();
        let live: HashSet<String> = self
            .catalog
            .lock()
            .map(|items| items.iter().map(|plugin| plugin.id.clone()).collect())
            .unwrap_or_default();

        let dead_workers: Vec<String> = self
            .workers
            .lock()
            .map(|workers| {
                workers
                    .keys()
                    .filter(|id| !live.contains(*id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for id in &dead_workers {
            self.stop(id);
        }

        let (states_removed, storage_removed) = {
            let mut states = self
                .states
                .lock()
                .map_err(|_| "插件状态不可用".to_string())?;
            let dead_ids: Vec<String> = states
                .plugins
                .keys()
                .filter(|id| !live.contains(*id))
                .cloned()
                .collect();
            for id in &dead_ids {
                states.plugins.remove(id);
            }
            if !dead_ids.is_empty() {
                states.save(&self.state_path)?;
            }

            let mut storage_removed = 0usize;
            if let Ok(entries) = std::fs::read_dir(&self.storage_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|value| value.to_str()) != Some("json") {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                        continue;
                    };
                    if !live.contains(stem) && std::fs::remove_file(&path).is_ok() {
                        storage_removed += 1;
                    }
                }
            }
            (dead_ids.len(), storage_removed)
        };

        if states_removed > 0 || storage_removed > 0 {
            self.log_line(format!(
                "[plugin] 清理死数据：state {states_removed} 条，storage {storage_removed} 个文件"
            ));
        }
        Ok(serde_json::json!({
            "states": states_removed,
            "storage": storage_removed,
            "workers": dead_workers.len(),
        }))
    }

    /// 记录启用状态并落盘。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        {
            let mut states = self
                .states
                .lock()
                .map_err(|_| "插件状态不可用".to_string())?;
            let state = states.get_mut(id);
            state.enabled = enabled;
            if enabled {
                state.errors = 0;
                state.last_error.clear();
            }
            states.save(&self.state_path)?;
        }
        if enabled {
            self.start(id);
        } else {
            self.stop(id);
        }
        Ok(())
    }

    /// 重新加载一个插件（先停后起）。
    pub fn reload(&self, id: &str) -> Result<(), String> {
        self.refresh_catalog();
        self.stop(id);
        let enabled = self
            .states
            .lock()
            .map(|states| states.state(id).enabled)
            .unwrap_or(true);
        if enabled {
            self.start(id);
        }
        Ok(())
    }

    pub fn attach_bridge(&self, bridge: Arc<dyn PluginBridge>) {
        if let Ok(mut slot) = self.bridge.lock() {
            *slot = Some(bridge);
        }
    }

    fn bridge(&self) -> Option<Arc<dyn PluginBridge>> {
        self.bridge.lock().ok().and_then(|slot| slot.clone())
    }

    /// 启动所有启用中的插件，并开始轮询目录 / 处理错误信号。
    pub fn start_all(&self) {
        self.refresh_catalog();
        let ids: Vec<String> = self
            .catalog
            .lock()
            .map(|items| items.iter().map(|plugin| plugin.id.clone()).collect())
            .unwrap_or_default();
        for id in ids {
            let enabled = self
                .states
                .lock()
                .map(|states| states.state(&id).enabled)
                .unwrap_or(true);
            if enabled {
                self.start(&id);
            }
        }
        self.spawn_poller();
    }

    fn spawn_poller(&self) {
        let dir = self.dir.clone();
        let catalog = self.catalog.clone();
        let workers = self.workers.clone();
        let states = self.states.clone();
        let log = self.log.clone();
        let bridge = self.bridge.clone();
        let storage_dir = self.storage_dir.clone();
        let state_path = self.state_path.clone();
        std::thread::Builder::new()
            .name("coconut-plugin-poller".into())
            .spawn(move || loop {
                std::thread::sleep(POLL_INTERVAL);
                materialize_builtins(&dir);
                let existing: Vec<String> = catalog
                    .lock()
                    .map(|items| items.iter().map(|plugin| plugin.id.clone()).collect())
                    .unwrap_or_default();
                let on_disk: Vec<String> = scan_plugins(&dir)
                    .into_iter()
                    .map(|plugin| plugin.id)
                    .collect();
                let changed = on_disk.iter().any(|id| !existing.contains(id))
                    || existing.iter().any(|id| !on_disk.contains(id));
                if changed {
                    // 新文件 / 被删文件：重建目录并起停对应的 worker
                    let mut next = scan_plugins(&dir);
                    for plugin in &mut next {
                        plugin.builtin = is_builtin_id(&plugin.id);
                    }
                    if let Ok(mut slot) = catalog.lock() {
                        *slot = next.clone();
                    }
                    let enabled_ids: Vec<String> = states
                        .lock()
                        .map(|states| {
                            next.iter()
                                .filter(|plugin| states.state(&plugin.id).enabled)
                                .map(|plugin| plugin.id.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Ok(mut running) = workers.lock() {
                        running.retain(|id, _| enabled_ids.contains(id));
                    }
                    // 新出现的插件：启一个 worker（之前的实现只更新列表，从不启动）
                    let Some(host) = bridge.lock().ok().and_then(|slot| slot.clone()) else {
                        continue;
                    };
                    for plugin in &next {
                        if !enabled_ids.contains(&plugin.id) {
                            continue;
                        }
                        let running = workers
                            .lock()
                            .map(|workers| workers.contains_key(&plugin.id))
                            .unwrap_or(false);
                        if running {
                            continue;
                        }
                        let (tx, rx) = std::sync::mpsc::channel::<Signal>();
                        let worker = plugin_rt::spawn(
                            plugin.clone(),
                            host.clone(),
                            tx,
                            plugin_rt::HOOK_TIMEOUT,
                        );
                        if let Ok(mut running) = workers.lock() {
                            running.insert(plugin.id.clone(), worker);
                        }
                        pump_signals(
                            plugin.id.clone(),
                            rx,
                            states.clone(),
                            workers.clone(),
                            state_path.clone(),
                            log.clone(),
                        );
                        log.line(&format!("[plugin] {} 已启动（轮询发现新文件）", plugin.id));
                    }
                    let _ = &storage_dir;
                }
            })
            .map(|_| ())
            .unwrap_or_else(|error| eprintln!("[plugin] 目录轮询线程启动失败：{error}"));
    }

    fn start(&self, id: &str) {
        let Some(bridge) = self.bridge() else {
            self.log_line(format!("[plugin] {id} 推迟启动：宿主还没准备好"));
            return;
        };
        let catalog = self
            .catalog
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default();
        let Some(plugin) = catalog.iter().find(|plugin| plugin.id == id).cloned() else {
            return;
        };
        if let Ok(mut running) = self.workers.lock() {
            if let Some(mut worker) = running.remove(id) {
                worker.stop();
            }
        }
        let (tx, rx) = std::sync::mpsc::channel::<Signal>();
        let worker = plugin_rt::spawn(plugin, bridge, tx, plugin_rt::HOOK_TIMEOUT);
        if let Ok(mut running) = self.workers.lock() {
            running.insert(id.to_string(), worker);
        }
        self.spawn_signal_pump(id.to_string(), rx);
        self.log_line(format!("[plugin] {id} 已启动"));
        let last_hello = self.last_hello.lock().ok().and_then(|slot| slot.clone());
        if let Some(info) = last_hello {
            if let Ok(running) = self.workers.lock() {
                if let Some(worker) = running.get(id) {
                    worker.hello(info);
                }
            }
        }
    }

    fn stop(&self, id: &str) {
        if let Ok(mut running) = self.workers.lock() {
            if let Some(mut worker) = running.remove(id) {
                worker.stop();
                self.log_line(format!("[plugin] {id} 已停止"));
            }
        }
    }

    fn spawn_signal_pump(&self, id: String, rx: Receiver<Signal>) {
        pump_signals(
            id,
            rx,
            self.states.clone(),
            self.workers.clone(),
            self.state_path.clone(),
            self.log.clone(),
        );
    }

    /// 工具箱页面就绪：广播给所有插件。
    pub fn hello(&self, info: Value) {
        if let Ok(mut slot) = self.last_hello.lock() {
            *slot = Some(info.clone());
        }
        if let Ok(workers) = self.workers.lock() {
            for worker in workers.values() {
                worker.hello(info.clone());
            }
        }
    }

    /// 原始 WS 帧：只发给声明了 `debug` 权限的插件。
    pub fn debug_frame(&self, frame: Value, outgoing: bool) {
        let debug_ids: Vec<String> = self
            .catalog
            .lock()
            .map(|items| {
                items
                    .iter()
                    .filter(|plugin| plugin.meta.has(Permission::Debug))
                    .map(|plugin| plugin.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if debug_ids.is_empty() {
            return;
        }
        let event = serde_json::json!({
            "type": "frame",
            "direction": if outgoing { "send" } else { "recv" },
            "frame": frame,
            "at": SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        if let Ok(workers) = self.workers.lock() {
            for id in debug_ids {
                if let Some(worker) = workers.get(&id) {
                    worker.event(event.clone());
                }
            }
        }
    }

    pub fn storage_path(&self, id: &str) -> PathBuf {
        self.storage_dir.join(format!("{id}.json"))
    }

    // ── 私有 KV ──
    fn storage_read(&self, id: &str) -> serde_json::Map<String, Value> {
        std::fs::read_to_string(self.storage_path(id))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    fn storage_write(&self, id: &str, map: &serde_json::Map<String, Value>) {
        let path = self.storage_path(id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            serde_json::to_vec_pretty(&Value::Object(map.clone())).unwrap_or_default(),
        );
    }

    pub fn storage_get(&self, id: &str, key: &str) -> String {
        self.storage_read(id)
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
            .to_string()
    }

    pub fn storage_set(&self, id: &str, key: &str, value: &str) {
        let mut map = self.storage_read(id);
        map.insert(
            key.to_string(),
            serde_json::from_str(value).unwrap_or(Value::Null),
        );
        self.storage_write(id, &map);
    }

    pub fn storage_delete(&self, id: &str, key: &str) {
        let mut map = self.storage_read(id);
        map.remove(key);
        self.storage_write(id, &map);
    }

    pub fn storage_all(&self, id: &str) -> String {
        Value::Object(self.storage_read(id)).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_exported_once_and_plugin_dir_wins() {
        let root = std::env::temp_dir().join(format!(
            "coconut-builtin-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        materialize_builtins(&root);
        let path = root.join("debug-terminal.js");
        assert!(path.is_file(), "built-in should be exported");
        std::fs::write(&path, "// user edited\n").expect("write override");
        materialize_builtins(&root);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "// user edited\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_dead_removes_state_and_storage_for_missing_plugins() {
        let root = std::env::temp_dir().join(format!(
            "coconut-dead-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let plugins = Plugins::new(&root);
        {
            let mut states = plugins.states.lock().unwrap();
            let state = states.get_mut("ghost");
            state.enabled = false;
            state.errors = 2;
            states.save(&plugins.state_path).expect("save state");
        }
        let storage = plugins.storage_path("ghost");
        std::fs::write(&storage, "{\"x\":1}").expect("write storage");
        let result = plugins.cleanup_dead().expect("cleanup");
        assert_eq!(result["states"], 1);
        assert_eq!(result["storage"], 1);
        let states = PluginStates::load(&plugins.state_path);
        assert!(states.plugins.get("ghost").is_none());
        assert!(!storage.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
