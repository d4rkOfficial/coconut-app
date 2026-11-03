//! 插件运行时：每个启用的插件跑在**自己的线程**上（QuickJS 不是线程安全的）。
//!
//! 关键设计：
//! - 宿主能力（runJs / callTool / fetch / storage）在 JS 里是**同步函数**，内部由插件线程
//!   阻塞等待 tokio 完成；所以插件可以照常写 `await Coconut.callTool(...)`，
//!   `await` 一个非 Promise 值只会多走一次微任务。
//! - 钩子可以是 `async function`：调用后由 JS 侧 `.then()` 打完成标记，Rust 泵微任务
//!   （`execute_pending_job`）直到完成 / 没有可跑的任务 / 超时。
//! - 超时靠 QuickJS 的中断回调：跑之前把 deadline 写进原子量，引擎定期回调检查。
//! - 定时器完全在 JS 侧（`setTimeout` / `setInterval` + `__coconut_fire_timers`），
//!   插件线程每 8ms 泵一次，够用且不用做跨语言定时器表。
//! - 权限在 Rust 侧强制：没有声明对应权限的 API 直接抛错，不是靠文档约束。

use crate::plugin::{Permission, Plugin};
use rquickjs::{Context, Function, Object, Runtime};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 单个钩子的最长执行时间。
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(10);
/// 连续失败这么多次就自动停用（成功一次清零）。
pub const MAX_CONSECUTIVE_ERRORS: u32 = 3;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 宿主能力。由 `server.rs` 实现（那里有 WS 会话、reqwest、磁盘）。
pub trait PluginBridge: Send + Sync {
    fn log(&self, plugin: &str, level: &str, message: &str);
    /// console.*：只进调试终端，不写日志文件。
    fn console_write(&self, _plugin: &str, _level: &str, _message: &str) {}
    /// console.clear()：清空调试终端。
    fn console_clear(&self) {}
    fn run_js(&self, plugin: &str, code: &str) -> Result<String, String>;
    fn call_tool(&self, plugin: &str, tool: &str, args: &str) -> Result<String, String>;
    fn fetch(&self, plugin: &str, url: &str, opts: &str) -> Result<String, String>;
    /// 注册一个浮层，返回引擎分配的浮层 id（失败时返回错误文本）
    fn ui_overlay(&self, plugin: &str, html: &str, css: &str) -> Result<u32, String>;
    fn ui_overlay_visible(&self, plugin: &str, id: u32, visible: bool) -> Result<bool, String>;
    fn ui_overlay_remove(&self, plugin: &str, id: u32) -> Result<bool, String>;
    fn ui_inject_css(&self, plugin: &str, id: &str, css: &str) -> Result<(), String>;
    /// 启动器信息：{ lang, toolboxVersion, gameVersion, src: { toolbox, static } }
    /// 其中 src.* 是 { dir, url }：本地目录路径 + 启动器本地服务地址。
    fn launcher_info(&self) -> Result<Value, String>;
    fn storage_get(&self, plugin: &str, key: &str) -> Result<String, String>;
    fn storage_set(&self, plugin: &str, key: &str, value: &str) -> Result<(), String>;
    fn storage_delete(&self, plugin: &str, key: &str) -> Result<(), String>;
    fn storage_all(&self, plugin: &str) -> Result<String, String>;
    /// 开一个 HTTP / WebSocket 服务（默认实现给测试用）。
    fn server_listen(&self, _plugin: &str, _host: &str, _port: u16) -> Result<Value, String> {
        Err("当前宿主不支持 server".into())
    }
    fn server_close(&self, _plugin: &str, _id: u64) -> Result<(), String> {
        Err("当前宿主不支持 server".into())
    }
    fn server_close_all(&self, _plugin: &str) {}
    fn server_respond(
        &self,
        _plugin: &str,
        _request: u64,
        _status: u16,
        _headers: &str,
        _body: &str,
        _end: bool,
    ) -> Result<bool, String> {
        Err("当前宿主不支持 server".into())
    }
    fn server_ws_send(
        &self,
        _plugin: &str,
        _server: u64,
        _connection: u64,
        _text: &str,
    ) -> Result<bool, String> {
        Err("当前宿主不支持 server".into())
    }
    fn server_ws_close(
        &self,
        _plugin: &str,
        _server: u64,
        _connection: u64,
    ) -> Result<bool, String> {
        Err("当前宿主不支持 server".into())
    }
    fn ws_client_connect(&self, _plugin: &str, _url: &str) -> Result<Value, String> {
        Err("当前宿主不支持 WebSocket".into())
    }
    fn ws_client_send(&self, _plugin: &str, _id: u64, _text: &str) -> Result<bool, String> {
        Err("当前宿主不支持 WebSocket".into())
    }
    fn ws_client_close(&self, _plugin: &str, _id: u64) -> Result<bool, String> {
        Err("当前宿主不支持 WebSocket".into())
    }
    fn poll_events(&self, _plugin: &str) -> Result<String, String> {
        Ok("[]".into())
    }
}

/// 插件线程回报给管理器的事件。
pub enum Signal {
    /// 钩子 / 定时器成功跑完一次（清零连续错误计数）
    Ok,
    /// 抛错（累计到 3 次自动停用）
    Error(String),
}

pub enum Job {
    Hello(Value),
    Event(Value),
    Stop,
}

/// 一个正在运行的插件。
pub struct Worker {
    pub id: String,
    tx: Sender<Job>,
    handle: Option<std::thread::JoinHandle<()>>,
    bridge: Arc<dyn PluginBridge>,
    servers_closed: bool,
}

impl Worker {
    pub fn hello(&self, info: Value) {
        let _ = self.tx.send(Job::Hello(info));
    }

    pub fn event(&self, event: Value) {
        let _ = self.tx.send(Job::Event(event));
    }

    pub fn stop(&mut self) {
        let _ = self.tx.send(Job::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.close_servers();
    }

    fn close_servers(&mut self) {
        if self.servers_closed {
            return;
        }
        self.servers_closed = true;
        self.bridge.server_close_all(&self.id);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.close_servers();
    }
}

/// 启动一个插件线程。源码在这一步就编译执行，语法错误会立刻回一个 `Signal::Error`。
pub fn spawn(
    plugin: Plugin,
    bridge: Arc<dyn PluginBridge>,
    signals: Sender<Signal>,
    timeout: Duration,
) -> Worker {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let id = plugin.id.clone();
    let thread_name = format!("coconut-plugin-{id}");
    let worker_bridge = bridge.clone();
    match std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || worker_main(plugin, bridge, signals, rx, timeout))
    {
        Ok(handle) => Worker {
            id,
            tx,
            handle: Some(handle),
            bridge: worker_bridge,
            servers_closed: false,
        },
        Err(error) => {
            eprintln!("[plugin] 启动插件线程失败：{error}");
            Worker {
                id,
                tx,
                handle: None,
                bridge: worker_bridge,
                servers_closed: false,
            }
        }
    }
}

fn worker_main(
    plugin: Plugin,
    bridge: Arc<dyn PluginBridge>,
    signals: Sender<Signal>,
    rx: Receiver<Job>,
    timeout: Duration,
) {
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = signals.send(Signal::Error(format!("创建 QuickJS 运行时失败：{error}")));
            return;
        }
    };
    let deadline = Arc::new(AtomicU64::new(0));
    {
        let deadline = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let value = deadline.load(Ordering::SeqCst);
            value != 0 && now_ms() > value
        })));
    }
    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => {
            let _ = signals.send(Signal::Error(format!("创建 QuickJS 上下文失败：{error}")));
            return;
        }
    };

    let permissions = plugin.meta.permissions.clone();
    let meta_json = json!({
        "id": plugin.id,
        "name": plugin.meta.name,
        "version": plugin.meta.version,
        "mount": plugin.meta.mount.as_str(),
        "permissions": plugin.meta.permissions.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
    })
    .to_string();

    if let Err(error) = install_api(
        &context,
        &plugin.id,
        &permissions,
        &bridge,
        &signals,
        &meta_json,
    ) {
        let _ = signals.send(Signal::Error(format!("安装插件 API 失败：{error}")));
        return;
    }

    if let Err(error) =
        context.with(|ctx| -> Result<(), rquickjs::Error> { ctx.eval::<(), _>(PRELUDE) })
    {
        let _ = signals.send(Signal::Error(format!("插件预置脚本失败：{error}")));
        return;
    }

    let source = plugin.source.clone();
    match context.with(|ctx| -> Result<(), rquickjs::Error> { ctx.eval::<(), _>(source) }) {
        Ok(()) => {
            let _ = signals.send(Signal::Ok);
        }
        Err(error) => {
            let detail = describe_exception(&context);
            let _ = signals.send(Signal::Error(if detail.is_empty() {
                format!("插件加载失败：{error}")
            } else {
                format!("插件加载失败：{detail}")
            }));
            return;
        }
    }

    // onStart：启动器已经起来了，但工具箱页面可能还没打开。
    report(
        &signals,
        run_hook(&context, &deadline, timeout, "onStart", &Value::Null),
    );

    loop {
        match rx.recv_timeout(Duration::from_millis(8)) {
            Ok(Job::Hello(info)) => {
                report(
                    &signals,
                    run_hook(&context, &deadline, timeout, "onHello", &info),
                );
            }
            Ok(Job::Event(event)) => {
                report(
                    &signals,
                    run_hook(&context, &deadline, timeout, "onEvent", &event),
                );
            }
            Ok(Job::Stop) => {
                let _ = run_hook(&context, &deadline, timeout, "onStop", &Value::Null);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = run_hook(&context, &deadline, timeout, "onStop", &Value::Null);
                break;
            }
        }

        // 定时器 + 由定时器回调产生的微任务
        deadline.store(now_ms() + timeout.as_millis() as u64, Ordering::SeqCst);
        let _ = context.with(|ctx| -> Result<(), rquickjs::Error> {
            let fire = ctx.globals().get::<_, Function>("__coconut_fire_timers")?;
            let _ = fire.call::<_, u32>(())?;
            Ok(())
        });
        // 服务 handler 里可能同步等一次上游请求（模型生成可能几十秒），
        // 不能用钩子的 10 秒上限；给它一个更宽的独立预算，避免把正常请求打断。
        deadline.store(
            now_ms().saturating_add(timeout.as_millis() as u64 * 30),
            Ordering::SeqCst,
        );
        pump_server_events(&context);
        pump_jobs(&context);
        deadline.store(0, Ordering::SeqCst);
    }
}

/// QuickJS 的 `Error::Exception` 只给一句「Exception generated by QuickJS」，
/// 真正的原因要用 `ctx.catch()` 取出来。
fn describe_exception(context: &Context) -> String {
    context.with(|ctx| -> String {
        let value = ctx.catch();
        if let Some(exception) = value.clone().into_exception() {
            if let Some(message) = exception.message() {
                return message;
            }
        }
        value
            .get::<rquickjs::String>()
            .ok()
            .and_then(|text| text.to_string().ok())
            .unwrap_or_default()
    })
}

fn report(signals: &Sender<Signal>, result: Result<(), String>) {
    match result {
        Ok(()) => {
            let _ = signals.send(Signal::Ok);
        }
        Err(error) => {
            let _ = signals.send(Signal::Error(error));
        }
    }
}

fn install_api(
    context: &Context,
    plugin_id: &str,
    permissions: &[Permission],
    bridge: &Arc<dyn PluginBridge>,
    signals: &Sender<Signal>,
    meta_json: &str,
) -> Result<(), rquickjs::Error> {
    context.with(|ctx| -> Result<(), rquickjs::Error> {
        let coconut = Object::new(ctx.clone())?;
        let globals = ctx.globals();

        macro_rules! need {
            ($perms:expr, $permission:expr, $label:expr) => {
                if !$perms.contains(&$permission) {
                    return error_envelope(&format!("插件缺少 {} 权限", $label));
                }
            };
        }

        // runJs
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "runJs",
                Function::new(ctx.clone(), move |code: String| -> String {
                    need!(perms, Permission::Dom, "dom");
                    match bridge.run_js(&id, &code) {
                        Ok(value) => ok_envelope(json!(value)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // callTool
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "callTool",
                Function::new(ctx.clone(), move |tool: String, args: String| -> String {
                    need!(perms, Permission::Toolbox, "toolbox");
                    match bridge.call_tool(&id, &tool, &args) {
                        Ok(value) => {
                            ok_envelope(serde_json::from_str(&value).unwrap_or(Value::Null))
                        }
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // fetch
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "fetch",
                Function::new(ctx.clone(), move |url: String, opts: String| -> String {
                    need!(perms, Permission::Network, "network");
                    match bridge.fetch(&id, &url, &opts) {
                        Ok(value) => {
                            ok_envelope(serde_json::from_str(&value).unwrap_or(Value::Null))
                        }
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // server：开本地 HTTP / WebSocket 服务
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "serverListen",
                Function::new(ctx.clone(), move |host: String, port: u32| -> String {
                    need!(perms, Permission::Server, "server");
                    match bridge.server_listen(&id, &host, port as u16) {
                        Ok(value) => ok_envelope(value),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "serverClose",
                Function::new(ctx.clone(), move |server: u64| -> String {
                    need!(perms, Permission::Server, "server");
                    match bridge.server_close(&id, server) {
                        Ok(()) => ok_envelope(Value::Null),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "serverRespond",
                Function::new(
                    ctx.clone(),
                    move |request: u64,
                          status: u32,
                          headers: String,
                          body: String,
                          end: bool|
                          -> String {
                        need!(perms, Permission::Server, "server");
                        match bridge.server_respond(
                            &id,
                            request,
                            status as u16,
                            &headers,
                            &body,
                            end,
                        ) {
                            Ok(value) => ok_envelope(json!(value)),
                            Err(error) => error_envelope(&error),
                        }
                    },
                )?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "serverWsSend",
                Function::new(
                    ctx.clone(),
                    move |server: u64, connection: u64, text: String| -> String {
                        need!(perms, Permission::Server, "server");
                        match bridge.server_ws_send(&id, server, connection, &text) {
                            Ok(value) => ok_envelope(json!(value)),
                            Err(error) => error_envelope(&error),
                        }
                    },
                )?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "serverWsClose",
                Function::new(ctx.clone(), move |server: u64, connection: u64| -> String {
                    need!(perms, Permission::Server, "server");
                    match bridge.server_ws_close(&id, server, connection) {
                        Ok(value) => ok_envelope(json!(value)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // WebSocket 客户端：走 network 权限
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "wsConnect",
                Function::new(ctx.clone(), move |url: String| -> String {
                    need!(perms, Permission::Network, "network");
                    match bridge.ws_client_connect(&id, &url) {
                        Ok(value) => ok_envelope(value),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "wsSend",
                Function::new(ctx.clone(), move |client: u64, text: String| -> String {
                    need!(perms, Permission::Network, "network");
                    match bridge.ws_client_send(&id, client, &text) {
                        Ok(value) => ok_envelope(json!(value)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "wsClose",
                Function::new(ctx.clone(), move |client: u64| -> String {
                    need!(perms, Permission::Network, "network");
                    match bridge.ws_client_close(&id, client) {
                        Ok(value) => ok_envelope(json!(value)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            coconut.set(
                "pollEvents",
                Function::new(ctx.clone(), move || -> String {
                    match bridge.poll_events(&id) {
                        Ok(value) => ok_envelope(serde_json::from_str(&value).unwrap_or(json!([]))),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // storage
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "getStorage",
                Function::new(ctx.clone(), move |key: String| -> String {
                    need!(perms, Permission::Storage, "storage");
                    match bridge.storage_get(&id, &key) {
                        Ok(raw) => ok_envelope(serde_json::from_str(&raw).unwrap_or(Value::Null)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "setStorage",
                Function::new(ctx.clone(), move |key: String, value: String| -> String {
                    need!(perms, Permission::Storage, "storage");
                    match bridge.storage_set(&id, &key, &value) {
                        Ok(()) => ok_envelope(Value::Null),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "deleteStorage",
                Function::new(ctx.clone(), move |key: String| -> String {
                    need!(perms, Permission::Storage, "storage");
                    match bridge.storage_delete(&id, &key) {
                        Ok(()) => ok_envelope(Value::Null),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "allStorage",
                Function::new(ctx.clone(), move || -> String {
                    need!(perms, Permission::Storage, "storage");
                    match bridge.storage_all(&id) {
                        Ok(raw) => ok_envelope(serde_json::from_str(&raw).unwrap_or(json!({}))),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // log
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            coconut.set(
                "log",
                Function::new(
                    ctx.clone(),
                    move |level: String, message: String| -> String {
                        bridge.log(&id, &level, &message);
                        ok_envelope(Value::Null)
                    },
                )?,
            )?;
        }
        // console.*：只进内置调试终端，不写日志文件
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            coconut.set(
                "consoleWrite",
                Function::new(
                    ctx.clone(),
                    move |level: String, message: String| -> String {
                        bridge.console_write(&id, &level, &message);
                        ok_envelope(Value::Null)
                    },
                )?,
            )?;
        }
        {
            let bridge = bridge.clone();
            coconut.set(
                "consoleClear",
                Function::new(ctx.clone(), move || -> String {
                    bridge.console_clear();
                    ok_envelope(Value::Null)
                })?,
            )?;
        }

        // ui：浮层注册 / 显隐 / 删除，外加页面级 CSS 注入
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "uiOverlay",
                Function::new(ctx.clone(), move |html: String, css: String| -> String {
                    need!(perms, Permission::Ui, "ui");
                    need!(perms, Permission::Dom, "dom");
                    match bridge.ui_overlay(&id, &html, &css) {
                        Ok(overlay) => json!({ "ok": true, "value": overlay }).to_string(),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "uiOverlayVisible",
                Function::new(ctx.clone(), move |overlay: u32, visible: bool| -> String {
                    need!(perms, Permission::Ui, "ui");
                    need!(perms, Permission::Dom, "dom");
                    match bridge.ui_overlay_visible(&id, overlay, visible) {
                        Ok(found) => json!({ "ok": true, "value": found }).to_string(),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "uiOverlayRemove",
                Function::new(ctx.clone(), move |overlay: u32| -> String {
                    need!(perms, Permission::Ui, "ui");
                    need!(perms, Permission::Dom, "dom");
                    match bridge.ui_overlay_remove(&id, overlay) {
                        Ok(found) => json!({ "ok": true, "value": found }).to_string(),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "uiInjectCss",
                Function::new(ctx.clone(), move |slot: String, css: String| -> String {
                    need!(perms, Permission::Ui, "ui");
                    need!(perms, Permission::Dom, "dom");
                    match bridge.ui_inject_css(&id, &slot, &css) {
                        Ok(()) => ok_envelope(Value::Null),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // 启动器信息 + 资源读取（不申请权限）
        {
            let bridge = bridge.clone();
            coconut.set(
                "launcherInfo",
                Function::new(ctx.clone(), move || -> String {
                    match bridge.launcher_info() {
                        Ok(info) => ok_envelope(info),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }
        let meta = meta_json.to_string();
        coconut.set(
            "meta",
            Function::new(ctx.clone(), move || -> String { meta.clone() })?,
        )?;

        // 未捕获异常 / Promise 拒绝 / 定时器回调抛错 → 交给管理器计数
        {
            let signals = signals.clone();
            let id = plugin_id.to_string();
            coconut.set(
                "reportError",
                Function::new(ctx.clone(), move |message: String| -> String {
                    let _ = signals.send(Signal::Error(format!("[{id}] {message}")));
                    ok_envelope(Value::Null)
                })?,
            )?;
        }

        globals.set("coconut", coconut)?;
        Ok(())
    })
}

/// 跑一个钩子：`None` 未定义时直接算成功。
fn run_hook(
    context: &Context,
    deadline: &Arc<AtomicU64>,
    timeout: Duration,
    name: &str,
    arg: &Value,
) -> Result<(), String> {
    let arg_json = arg.to_string();
    let limit = now_ms().saturating_add(timeout.as_millis() as u64);
    deadline.store(limit, Ordering::SeqCst);

    let started = context.with(|ctx| -> Result<bool, rquickjs::Error> {
        ctx.eval::<(), _>(format!("__coconut_hook_start({})", json!(name)))?;
        let invoke = ctx.globals().get::<_, Function>("__coconut_hook_invoke")?;
        invoke.call::<_, bool>((arg_json,))
    });

    let pending = match started {
        Ok(pending) => pending,
        Err(error) => {
            deadline.store(0, Ordering::SeqCst);
            return Err(format!("{name} 调用失败：{error}"));
        }
    };

    let mut timed_out = false;
    let mut idle_rounds = 0;
    if pending {
        loop {
            if now_ms() > limit {
                timed_out = true;
                break;
            }
            // 钩子等的是定时器：`await sleep(ms)` 必须能把时间推过去，
            // 否则异步钩子里任何等待都会变成「挂起」。
            let _ = context.with(|ctx| -> Result<(), rquickjs::Error> {
                let fire = ctx.globals().get::<_, Function>("__coconut_fire_timers")?;
                let _ = fire.call::<_, u32>(())?;
                Ok(())
            });
            let done = context
                .with(|ctx| -> Result<bool, rquickjs::Error> {
                    let done = ctx
                        .globals()
                        .get::<_, Function>("__coconut_hook_done")?
                        .call::<_, bool>(())?;
                    Ok(done)
                })
                .unwrap_or(true);
            if done {
                break;
            }
            let advanced = context.with(|ctx| ctx.execute_pending_job());
            if !advanced {
                // 没有可执行的微任务：大概率是在等定时器（`await sleep(ms)`），
                // 直接睡到下一个定时器到点；只有「一个定时器都没有」才算挂起。
                let next_due = context.with(|ctx| -> f64 {
                    ctx.globals()
                        .get::<_, Function>("__coconut_next_timer")
                        .and_then(|function| function.call::<_, f64>(()))
                        .unwrap_or(-1.0)
                });
                if next_due >= 0.0 {
                    let wait = (next_due - now_ms() as f64).clamp(1.0, 50.0);
                    std::thread::sleep(Duration::from_millis(wait as u64));
                    idle_rounds = 0;
                    continue;
                }
                idle_rounds += 1;
                if idle_rounds > 40 {
                    deadline.store(0, Ordering::SeqCst);
                    return Err(format!(
                        "{name} 挂起：没有可执行的微任务（是不是 await 了一个不会完成的 Promise？）"
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            idle_rounds = 0;
        }
    }
    deadline.store(0, Ordering::SeqCst);

    if timed_out {
        return Err(format!("{name} 超过 {}s 未返回，已中断", timeout.as_secs()));
    }

    let state = context
        .with(|ctx| -> Result<String, rquickjs::Error> {
            let state = ctx
                .globals()
                .get::<_, Function>("__coconut_hook_state")?
                .call::<_, String>(())?;
            Ok(state)
        })
        .ok()
        .and_then(|state| serde_json::from_str::<Value>(&state).ok());

    match state {
        Some(state) => match state["error"].as_str() {
            Some(error) if !error.is_empty() => Err(format!("{name} 抛错：{error}")),
            _ => Ok(()),
        },
        None => Ok(()),
    }
}

/// 取插件服务 / WebSocket 客户端的待处理事件，并在 JS 里分发给监听器。
fn pump_server_events(context: &Context) {
    let _ = context.with(|ctx| -> Result<(), rquickjs::Error> {
        if let Ok(poll) = ctx.globals().get::<_, Function>("__coconut_poll_events") {
            let _ = poll.call::<_, u32>(());
        }
        Ok(())
    });
}

fn pump_jobs(context: &Context) -> bool {
    let mut ran = false;
    for _ in 0..64 {
        if context.with(|ctx| ctx.execute_pending_job()) {
            ran = true;
        } else {
            break;
        }
    }
    ran
}

fn ok_envelope(value: Value) -> String {
    json!({ "ok": true, "value": value }).to_string()
}

fn error_envelope(message: &str) -> String {
    json!({ "ok": false, "error": message }).to_string()
}

/// 页面侧预置脚本：/// - 把 `coconut.*` 的 `{ok, value}` 信封拆成返回值或抛错；
/// - `Coconut` 对象（runJs / callTool / fetch / storage / ui / log）；
/// - `setTimeout` / `setInterval`（由 Rust 每 8ms 调 `__coconut_fire_timers`）；
/// - 钩子驱动 `__coconut_hook_start` / `__coconut_hook_invoke` / `__coconut_hook_done` / `__coconut_hook_state`。
const PRELUDE: &str = r#"
(function () {
  'use strict';

  function unwrap(raw) {
    const envelope = JSON.parse(String(raw));
    if (!envelope || envelope.ok !== true) {
      throw new Error((envelope && envelope.error) || 'host call failed');
    }
    return envelope.value;
  }

  const meta = JSON.parse(String(coconut.meta()));

  let timerSeq = 0;
  const timers = new Map();
  function schedule(fn, ms, repeat) {
    if (typeof fn !== 'function') throw new TypeError('callback must be a function');
    const id = ++timerSeq;
    const every = Math.max(0, Number(ms) || 0);
    timers.set(id, { fn: fn, due: Date.now() + every, repeat: repeat ? Math.max(1, every) : 0 });
    return id;
  }
  function cancel(id) { timers.delete(id); }
  globalThis.setTimeout = function (fn, ms) { return schedule(fn, ms, false); };
  globalThis.setInterval = function (fn, ms) { return schedule(fn, ms, true); };
  globalThis.clearTimeout = cancel;
  globalThis.clearInterval = cancel;
  globalThis.__coconut_fire_timers = function () {
    const now = Date.now();
    let fired = 0;
    for (const entry of Array.from(timers.entries())) {
      const id = entry[0];
      const timer = entry[1];
      if (timer.due > now) continue;
      if (timer.repeat) timer.due = now + timer.repeat;
      else timers.delete(id);
      fired++;
      try {
        timer.fn();
      } catch (error) {
        coconut.reportError(String((error && error.stack) || error));
      }
    }
    return fired;
  };

  function createEmitter() {
    const listeners = Object.create(null);
    return {
      addEventListener: function (type, listener) {
        if (typeof listener !== 'function') throw new TypeError('listener must be a function');
        const key = String(type || '');
        const list = listeners[key] || (listeners[key] = []);
        if (list.indexOf(listener) < 0) list.push(listener);
        return this;
      },
      removeEventListener: function (type, listener) {
        const list = listeners[String(type || '')];
        if (!list) return this;
        const index = list.indexOf(listener);
        if (index >= 0) list.splice(index, 1);
        return this;
      },
      __listeners: function (type) {
        return (listeners[String(type || '')] || []).slice();
      }
    };
  }

  const Coconut = createEmitter();
  Object.assign(Coconut, {
    plugin: meta,
    runJs: function (code) { return String(unwrap(coconut.runJs(String(code)))); },
    callTool: function (tool, args) {
      return unwrap(coconut.callTool(String(tool), JSON.stringify(args === undefined ? {} : args)));
    },
    fetch: function (url, options) {
      return unwrap(coconut.fetch(String(url), JSON.stringify(options || {})));
    },
    storage: {
      get: function (key, fallback) {
        const value = unwrap(coconut.getStorage(String(key)));
        return value === null || value === undefined ? (fallback === undefined ? null : fallback) : value;
      },
      set: function (key, value) { unwrap(coconut.setStorage(String(key), JSON.stringify(value))); return true; },
      delete: function (key) { unwrap(coconut.deleteStorage(String(key))); return true; },
      all: function () { return unwrap(coconut.allStorage()) || {}; }
    },
    ui: {
      /** 注册浮层：html + css 给引擎，返回浮层 id（从 1 递增）。 */
      overlay: function (options) {
        var opts = options === undefined || options === null ? {} : options;
        var html = typeof opts === "string" ? opts : (opts.html === undefined || opts.html === null ? "" : String(opts.html));
        var css = typeof opts === "string" ? "" : (opts.css === undefined || opts.css === null ? "" : String(opts.css));
        return unwrap(coconut.uiOverlay(String(html), String(css)));
      },
      /** 显示 / 隐藏浮层；返回是否找到了这个 id。 */
      visible: function (id, visible) {
        return Boolean(unwrap(coconut.uiOverlayVisible(Number(id), Boolean(visible))));
      },
      show: function (id) { return Coconut.ui.visible(id, true); },
      hide: function (id) { return Coconut.ui.visible(id, false); },
      /** 删除浮层；返回是否找到了这个 id。 */
      remove: function (id) {
        return Boolean(unwrap(coconut.uiOverlayRemove(Number(id))));
      },
      injectCss: function (slot, css) { unwrap(coconut.uiInjectCss(String(slot), String(css))); return true; }
    },
    /** 等一会儿（毫秒）。插件里写 await Coconut.sleep(500) 就行。 */
    launcher: (function () {
      const live = {};
      const readInfo = function () {
        try {
          return unwrap(coconut.launcherInfo()) || {};
        } catch (error) {
          return {};
        }
      };
      const keys = ["lang", "toolboxVersion", "gameVersion"];
      for (let i = 0; i < keys.length; i++) {
        (function (key) {
          Object.defineProperty(live, key, {
            enumerable: true,
            get: function () {
              const info = readInfo();
              return key === "lang" ? String(info.lang || "en") : String(info[key] || "");
            }
          });
        })(keys[i]);
      }
      return live;
    })(),
    src: (function () {
      const info = unwrap(coconut.launcherInfo()) || {};
      const roots = info.src || {};
      const pick = function (entry) {
        return {
          dir: String((entry && entry.dir) || ""),
          url: String((entry && entry.url) || "")
        };
      };
      return {
        toolbox: pick(roots.toolbox),
        static: pick(roots.static)
      };
    })(),
    /** 等一会儿（毫秒）：插件里写 await Coconut.sleep(500) 就行。 */
    sleep: function (ms) {
      var delay = Number(ms);
      if (!isFinite(delay) || delay < 0) delay = 0;
      return new Promise(function (resolve) {
        globalThis.setTimeout(function () { resolve(); }, delay);
      });
    },
    log: function () {
      const parts = [];
      for (let i = 0; i < arguments.length; i++) parts.push(String(arguments[i]));
      return coconut.log('info', parts.join(' '));
    },
    setTimeout: globalThis.setTimeout,
    setInterval: globalThis.setInterval,
    clearTimeout: cancel,
    clearInterval: cancel
  });

  const httpServers = new Map();
  const serverSockets = new Map();
  const webSockets = new Map();

  function safeReport(error) {
    try {
      coconut.reportError(String((error && error.stack) || error));
    } catch (ignored) {}
  }

  function emit(target, type, event) {
    const listeners = target.__listeners(type);
    for (let i = 0; i < listeners.length; i++) {
      try {
        const result = listeners[i](event);
        if (result && typeof result.then === 'function') result.catch(safeReport);
      } catch (error) {
        safeReport(error);
      }
    }
  }

  function responseFor(requestId) {
    let status = 200;
    let headers = { 'content-type': 'text/plain; charset=utf-8' };
    let ended = false;
    const flush = function (body, end) {
      if (ended) return false;
      const text = body === undefined || body === null ? '' : String(body);
      let value = false;
      try {
        value = Boolean(unwrap(coconut.serverRespond(Number(requestId), Number(status), JSON.stringify(headers), text, Boolean(end))));
      } catch (error) {
        ended = true;
        throw error;
      }
      if (end) ended = true;
      return value;
    };
    const response = {
      get ended() { return ended; },
      status: function (code) { status = Number(code) || 200; return response; },
      header: function (name, value) { headers[String(name)] = String(value); return response; },
      setHeader: function (name, value) { return response.header(name, value); },
      write: function (chunk) { return flush(chunk, false); },
      end: function (body) { return flush(body, true); },
      send: function (body) {
        if (body && typeof body === 'object' && !(body instanceof String)) {
          headers['content-type'] = headers['content-type'] || 'application/json; charset=utf-8';
          return flush(JSON.stringify(body), true);
        }
        return flush(body, true);
      },
      json: function (value) {
        headers['content-type'] = 'application/json; charset=utf-8';
        return flush(JSON.stringify(value), true);
      }
    };
    return response;
  }

  function socketFor(server, connection, path) {
    const key = String(server) + ':' + String(connection);
    const existing = serverSockets.get(key);
    if (existing) return existing;
    const socket = createEmitter();
    socket.id = connection;
    socket.path = path || '';
    socket.send = function (data) {
      return Boolean(unwrap(coconut.serverWsSend(Number(server), Number(connection), String(data))));
    };
    socket.close = function () {
      return Boolean(unwrap(coconut.serverWsClose(Number(server), Number(connection))));
    };
    serverSockets.set(key, socket);
    return socket;
  }

  function dispatchHttp(event) {
    const server = httpServers.get(Number(event.server));
    if (!server) return;
    const response = responseFor(event.id);
    const request = {
      server: Number(event.server),
      method: String(event.method || 'GET'),
      path: String(event.path || '/'),
      query: String(event.query || ''),
      headers: event.headers || {},
      body: String(event.body || '')
    };
    const listeners = server.__listeners('request');
    if (!listeners.length) {
      response.status(404).send('no request listener');
      return;
    }
    for (let i = 0; i < listeners.length; i++) {
      try {
        const result = listeners[i](request, response);
        if (result && typeof result.then === 'function') {
          result.catch(function (error) {
            if (!response.ended) response.status(500).send('plugin handler failed: ' + String((error && error.message) || error));
            safeReport(error);
          });
        }
      } catch (error) {
        if (!response.ended) response.status(500).send('plugin handler failed: ' + String((error && error.message) || error));
        safeReport(error);
      }
    }
  }

  function dispatchRealtime(event) {
    const type = String(event.type || '');
    if (type === 'http-request') {
      dispatchHttp(event);
      return;
    }
    if (event.source === 'server') {
      const server = httpServers.get(Number(event.server));
      if (!server) return;
      if (type === 'ws-open') {
        const socket = socketFor(event.server, event.conn, event.path);
        emit(server, 'connection', socket);
        return;
      }
      const socket = serverSockets.get(String(event.server) + ':' + String(event.conn));
      if (!socket) return;
      if (type === 'ws-message') {
        emit(socket, 'message', { data: String(event.data || '') });
      } else if (type === 'ws-close') {
        emit(socket, 'close', { code: Number(event.code || 1005) });
        serverSockets.delete(String(event.server) + ':' + String(event.conn));
      }
      return;
    }
    if (event.source === 'client') {
      const socket = webSockets.get(Number(event.client));
      if (!socket) return;
      if (type === 'ws-open') {
        socket.connected = true;
        emit(socket, 'open', {});
      } else if (type === 'ws-message') {
        emit(socket, 'message', { data: String(event.data || '') });
      } else if (type === 'ws-close') {
        socket.connected = false;
        emit(socket, 'close', { code: Number(event.code || 1005) });
        webSockets.delete(Number(event.client));
      }
    }
  }

  globalThis.__coconut_poll_events = function () {
    let events;
    try {
      events = unwrap(coconut.pollEvents());
    } catch (error) {
      safeReport(error);
      return 0;
    }
    if (!Array.isArray(events)) return 0;
    for (let i = 0; i < events.length; i++) {
      try {
        dispatchRealtime(events[i]);
      } catch (error) {
        safeReport(error);
      }
    }
    return events.length;
  };

  const createHttpServer = function (options) {
    const opts = options === undefined || options === null ? {} : options;
    const host = String(opts.host === undefined || opts.host === null ? '127.0.0.1' : opts.host);
    const port = Number(opts.port || 0);
    const info = unwrap(coconut.serverListen(host, port));
    const server = createEmitter();
    server.id = Number(info.id);
    server.port = Number(info.port);
    server.close = function () {
      try {
        unwrap(coconut.serverClose(server.id));
      } finally {
        httpServers.delete(server.id);
        for (const key of Array.from(serverSockets.keys())) {
          if (key.indexOf(String(server.id) + ':') === 0) serverSockets.delete(key);
        }
      }
      return true;
    };
    httpServers.set(server.id, server);
    return server;
  };

  Coconut.http = function (options) {
    return createHttpServer(options);
  };
  Coconut.http.createServer = createHttpServer;

  Coconut.webSocket = function (url) {
    if (typeof url !== 'string' || !url) throw new TypeError('webSocket(url) 需要一个地址');
    const info = unwrap(coconut.wsConnect(url));
    const id = Number(info.id);
    const socket = createEmitter();
    socket.id = id;
    socket.url = url;
    socket.connected = true;
    socket.send = function (data) {
      return Boolean(unwrap(coconut.wsSend(id, String(data))));
    };
    socket.close = function () {
      try {
        return Boolean(unwrap(coconut.wsClose(id)));
      } finally {
        webSockets.delete(id);
      }
    };
    webSockets.set(id, socket);
    return socket;
  };

  globalThis.Coconut = Coconut;

  const levels = ['log', 'info', 'warn', 'error', 'debug'];
  globalThis.console = {};
  levels.forEach(function (level) {
    globalThis.console[level] = function () {
      const parts = [];
      for (let i = 0; i < arguments.length; i++) {
        const value = arguments[i];
        parts.push(typeof value === 'string' ? value : JSON.stringify(value));
      }
      coconut.consoleWrite(level === 'error' || level === 'warn' ? level : 'info', parts.join(' '));
    };
  });
  globalThis.console.clear = function () {
    coconut.consoleClear();
  };
  globalThis.console.dir = globalThis.console.log;
  globalThis.console.table = globalThis.console.log;

  let done = true;
  let error = '';
  let current = '';

  globalThis.__coconut_hook_start = function (name) {
    current = String(name);
    done = false;
    error = '';
  };

  globalThis.__coconut_hook_invoke = function (argJson) {
    const fn = globalThis[current];
    const key = String(current).replace(/^on/, '').toLowerCase();
    const listeners = Coconut.__listeners(key);
    if (typeof fn !== 'function' && !listeners.length) { done = true; return false; }
    const finish = function (reason) {
      if (reason !== undefined) {
        if (typeof reason === 'string') error = reason;
        else {
          const message = (reason && reason.message) ? String(reason.message) : String(reason);
          const stack = (reason && reason.stack) ? String(reason.stack) : '';
          error = stack ? message + '\n' + stack : message;
        }
      }
      done = true;
    };
    try {
      const arg = argJson ? JSON.parse(argJson) : null;
      const jobs = [];
      if (typeof fn === 'function') jobs.push(fn(arg));
      for (let i = 0; i < listeners.length; i++) jobs.push(listeners[i](arg));
      let pending = false;
      for (let i = 0; i < jobs.length; i++) {
        if (jobs[i] && typeof jobs[i].then === 'function') pending = true;
      }
      if (pending) {
        Promise.all(jobs).then(function () { finish(); }, function (reason) { finish(reason); });
        return true;
      }
      finish();
      return false;
    } catch (thrown) {
      finish(thrown);
      return false;
    }
  };

  globalThis.__coconut_hook_done = function () { return done; };
  globalThis.__coconut_has_timers = function () { return timers.size > 0; };
  globalThis.__coconut_next_timer = function () {
    let next = -1;
    for (const timer of timers.values()) {
      if (next < 0 || timer.due < next) next = timer.due;
    }
    return next;
  };
  globalThis.__coconut_hook_state = function () { return JSON.stringify({ done: done, error: error }); };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Plugin;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        logs: Mutex<Vec<String>>,
        tools: Mutex<Vec<String>>,
        fetches: Mutex<Vec<String>>,
        panels: Mutex<Vec<String>>,
        servers: Mutex<Vec<String>>,
        responses: Mutex<Vec<String>>,
        console: Mutex<Vec<String>>,
        clears: Mutex<u32>,
        ws_connections: Mutex<Vec<String>>,
        ws_sent: Mutex<Vec<String>>,
        events: Mutex<Vec<Value>>,
        storage: Mutex<HashMap<String, String>>,
    }

    impl Recorder {
        fn logs(&self) -> Vec<String> {
            self.logs.lock().unwrap().clone()
        }
        fn tools(&self) -> Vec<String> {
            self.tools.lock().unwrap().clone()
        }
        fn fetched(&self) -> Vec<String> {
            self.fetches.lock().unwrap().clone()
        }
        fn panels(&self) -> Vec<String> {
            self.panels.lock().unwrap().clone()
        }
        fn responses(&self) -> Vec<String> {
            self.responses.lock().unwrap().clone()
        }
        fn events(&self) -> &Mutex<Vec<Value>> {
            &self.events
        }
    }

    impl PluginBridge for Recorder {
        fn log(&self, _plugin: &str, _level: &str, message: &str) {
            self.logs.lock().unwrap().push(message.to_string());
        }

        fn console_write(&self, _plugin: &str, level: &str, message: &str) {
            self.console
                .lock()
                .unwrap()
                .push(format!("{level}:{message}"));
        }

        fn console_clear(&self) {
            *self.clears.lock().unwrap() += 1;
        }
        fn run_js(&self, _plugin: &str, _code: &str) -> Result<String, String> {
            Ok("{\"ok\":1}".to_string())
        }
        fn call_tool(&self, _plugin: &str, tool: &str, args: &str) -> Result<String, String> {
            self.tools.lock().unwrap().push(format!("{tool} {args}"));
            Ok("{\"echo\":true}".to_string())
        }
        fn fetch(&self, _plugin: &str, url: &str, _opts: &str) -> Result<String, String> {
            self.fetches.lock().unwrap().push(url.to_string());
            Ok("{\"data\":[]}".to_string())
        }
        fn ui_overlay(&self, _plugin: &str, html: &str, css: &str) -> Result<u32, String> {
            let mut panels = self.panels.lock().unwrap();
            panels.push(format!("{html} {css}"));
            Ok(panels.len() as u32)
        }
        fn ui_overlay_visible(
            &self,
            _plugin: &str,
            _id: u32,
            _visible: bool,
        ) -> Result<bool, String> {
            Ok(true)
        }
        fn ui_overlay_remove(&self, _plugin: &str, _id: u32) -> Result<bool, String> {
            Ok(true)
        }
        fn ui_inject_css(&self, _plugin: &str, _slot: &str, _css: &str) -> Result<(), String> {
            Ok(())
        }
        fn launcher_info(&self) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({
                "lang": "zh-Hans",
                "toolboxVersion": "test",
                "gameVersion": "0",
                "src": {
                    "toolbox": { "dir": ".", "url": "http://127.0.0.1:47821/" },
                    "static": { "dir": ".", "url": "http://127.0.0.1:47821/icons/" }
                }
            }))
        }
        fn storage_get(&self, _plugin: &str, key: &str) -> Result<String, String> {
            Ok(self
                .storage
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or_else(|| "null".to_string()))
        }
        fn storage_set(&self, _plugin: &str, key: &str, value: &str) -> Result<(), String> {
            self.storage
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn storage_delete(&self, _plugin: &str, key: &str) -> Result<(), String> {
            self.storage.lock().unwrap().remove(key);
            Ok(())
        }
        fn storage_all(&self, _plugin: &str) -> Result<String, String> {
            let map = self.storage.lock().unwrap().clone();
            Ok(serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string()))
        }

        fn server_listen(&self, _plugin: &str, host: &str, port: u16) -> Result<Value, String> {
            self.servers.lock().unwrap().push(format!("{host}:{port}"));
            Ok(json!({ "id": 1, "port": 12345 }))
        }

        fn server_respond(
            &self,
            _plugin: &str,
            request: u64,
            status: u16,
            headers: &str,
            body: &str,
            end: bool,
        ) -> Result<bool, String> {
            self.responses
                .lock()
                .unwrap()
                .push(format!("{request} {status} {headers} {body} {end}"));
            Ok(true)
        }

        fn poll_events(&self, _plugin: &str) -> Result<String, String> {
            let mut events = self.events.lock().unwrap();
            let out = Value::Array(events.drain(..).collect());
            Ok(out.to_string())
        }

        fn ws_client_connect(&self, _plugin: &str, url: &str) -> Result<Value, String> {
            self.ws_connections.lock().unwrap().push(url.to_string());
            Ok(json!({ "id": 9 }))
        }

        fn ws_client_send(&self, _plugin: &str, id: u64, text: &str) -> Result<bool, String> {
            self.ws_sent.lock().unwrap().push(format!("{id} {text}"));
            Ok(true)
        }
    }

    fn plugin(source: &str, permissions: &[Permission]) -> Plugin {
        let mut meta = crate::plugin::parse_meta(source, "test");
        meta.permissions = permissions.to_vec();
        Plugin {
            id: "test".to_string(),
            path: None,
            source: source.to_string(),
            meta,
            builtin: false,
        }
    }

    fn drain(rx: &Receiver<Signal>, wait: Duration) -> (Vec<String>, Vec<String>) {
        let mut ok = 0;
        let mut errors = Vec::new();
        let deadline = std::time::Instant::now() + wait;
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(80)) {
                Ok(Signal::Ok) => ok += 1,
                Ok(Signal::Error(error)) => errors.push(error),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        (vec![format!("ok:{ok}")], errors)
    }

    #[test]
    fn runs_hooks_and_bridges_host_calls() {
        let source = r#"
function onStart() {
  Coconut.log('started', 42);
  Coconut.storage.set('seen', 1);
  Coconut.callTool('get_ai_profiles', {});
}
async function onHello(info) {
  await Coconut.callTool('set_setting', { key: 'a', value: true });
  Coconut.log('hello ' + info.session);
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(
                source,
                &[Permission::Toolbox, Permission::Storage, Permission::Dom],
            ),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        std::thread::sleep(Duration::from_millis(150));
        worker.hello(json!({ "session": "s1" }));
        let (_, errors) = drain(&rx, Duration::from_millis(600));
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        let logs = recorder.logs();
        assert!(
            logs.iter().any(|line| line == "started 42"),
            "logs: {logs:?}"
        );
        assert!(
            logs.iter().any(|line| line == "hello s1"),
            "async hook did not resume after await: {logs:?}"
        );
        let tools = recorder.tools();
        assert_eq!(tools.len(), 2, "tools: {tools:?}");
        worker.stop();
    }

    #[test]
    fn lifecycle_listeners_run_alongside_global_hooks() {
        let source = r#"
async function onStart() {
  Coconut.log('hook-start');
  Coconut.addEventListener('stop', function () {
    Coconut.log('listener-stop');
  });
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let _ = drain(&rx, Duration::from_millis(200));
        worker.stop();
        let logs = recorder.logs();
        assert!(
            logs.iter().any(|line| line == "hook-start"),
            "logs: {logs:?}"
        );
        assert!(
            logs.iter().any(|line| line == "listener-stop"),
            "listener did not run: {logs:?}"
        );
    }

    #[test]
    fn http_server_events_reach_the_js_handler() {
        let source = r#"
function onStart() {
  Coconut.log('started');
  const server = Coconut.http.createServer({ port: 0 });
  server.addEventListener('request', function (request, response) {
    Coconut.log('request', request.path);
    response.status(202).json({ path: request.path, body: request.body });
  });
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Server]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        std::thread::sleep(Duration::from_millis(120));
        recorder.events().lock().unwrap().push(json!({
            "type": "http-request",
            "server": 1,
            "id": 7,
            "method": "POST",
            "path": "/x",
            "query": "",
            "headers": {},
            "body": "{\"a\":1}"
        }));
        let deadline = std::time::Instant::now() + Duration::from_millis(1000);
        while std::time::Instant::now() < deadline && recorder.responses().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let responses = recorder.responses();
        assert_eq!(
            responses.len(),
            1,
            "responses: {responses:?}; logs: {:?}; servers: {:?}",
            recorder.logs(),
            recorder.servers.lock().unwrap()
        );
        assert!(responses[0].contains("7 202"), "responses: {responses:?}");
        assert!(responses[0].contains("/x"), "responses: {responses:?}");
        let _ = drain(&rx, Duration::from_millis(50));
        worker.stop();
    }

    #[test]
    fn websocket_client_uses_the_event_object_api() {
        let source = r#"
function onStart() {
  const ws = Coconut.webSocket('ws://example.test');
  ws.addEventListener('open', function () {
    ws.send('hello');
  });
  ws.addEventListener('message', function (event) {
    Coconut.log('got ' + event.data);
  });
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Network]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        std::thread::sleep(Duration::from_millis(120));
        recorder.events().lock().unwrap().push(json!({
            "type": "ws-open",
            "source": "client",
            "client": 9
        }));
        recorder.events().lock().unwrap().push(json!({
            "type": "ws-message",
            "source": "client",
            "client": 9,
            "data": "world"
        }));
        let deadline = std::time::Instant::now() + Duration::from_millis(1000);
        while std::time::Instant::now() < deadline && recorder.ws_sent.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            recorder.ws_connections.lock().unwrap().as_slice(),
            ["ws://example.test"]
        );
        assert_eq!(recorder.ws_sent.lock().unwrap().as_slice(), ["9 hello"]);
        assert!(
            recorder.logs().iter().any(|line| line == "got world"),
            "logs: {:?}",
            recorder.logs()
        );
        let _ = drain(&rx, Duration::from_millis(50));
        worker.stop();
    }

    #[test]
    fn console_goes_to_terminal_bus_and_not_log_file() {
        let source = r#"
function onStart() {
  Coconut.log('coconut-log');
  console.log('terminal-line');
  console.clear();
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let _ = drain(&rx, Duration::from_millis(300));
        assert!(recorder.logs().iter().any(|line| line == "coconut-log"));
        assert_eq!(
            recorder.console.lock().unwrap().as_slice(),
            ["info:terminal-line"]
        );
        assert_eq!(*recorder.clears.lock().unwrap(), 1);
        assert!(!recorder.logs().iter().any(|line| line == "terminal-line"));
        worker.stop();
    }

    #[test]
    fn plugin_layer_eval_is_available() {
        let source = "function onStart() { Coconut.log(String(eval('1 + 1'))); }";
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let _ = drain(&rx, Duration::from_millis(300));
        assert!(recorder.logs().iter().any(|line| line == "2"));
        worker.stop();
    }

    #[test]
    fn missing_permission_throws_instead_of_silently_working() {
        let source = r#"
function onStart() {
  Coconut.fetch('https://example.com');
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let (_, errors) = drain(&rx, Duration::from_millis(600));
        assert!(
            errors.iter().any(|error| error.contains("network")),
            "errors: {errors:?}"
        );
        assert!(recorder.fetched().is_empty());
        worker.stop();
    }

    #[test]
    fn busy_loop_is_interrupted_by_the_timeout() {
        let source = "function onStart() { while (true) {} }";
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_millis(300),
        );
        let (_, errors) = drain(&rx, Duration::from_millis(1500));
        assert!(
            errors.iter().any(|error| error.contains("onStart")),
            "errors: {errors:?}"
        );
        worker.stop();
    }

    #[test]
    fn missing_hooks_are_allowed() {
        let source = "var unused = 1;";
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let (ok, errors) = drain(&rx, Duration::from_millis(400));
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(ok[0] != "ok:0", "no ok signal: {ok:?}");
        worker.stop();
    }

    #[test]
    fn syntax_error_reports_immediately() {
        let source = "function onStart( {";
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let (_, errors) = drain(&rx, Duration::from_millis(500));
        assert!(
            errors.iter().any(|error| error.contains("加载失败")),
            "errors: {errors:?}"
        );
        worker.stop();
    }

    #[test]
    fn ui_and_timer_apis_reach_the_bridge() {
        let source = r#"
async function onStart() {
  let ticks = 0;
  const id = setInterval(function () {
    ticks += 1;
    if (ticks >= 2) {
      clearInterval(id);
      const overlayId = Coconut.ui.overlay({ html: '<b>X</b>' });
      Coconut.ui.hide(overlayId);
    }
  }, 20);
  await Coconut.sleep(80);
}
"#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Ui, Permission::Dom]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(1200);
        while std::time::Instant::now() < deadline && recorder.panels().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let panels = recorder.panels();
        assert_eq!(panels.len(), 1, "panels: {panels:?}");
        assert!(panels[0].starts_with("<b>X</b>"), "panels: {panels:?}");
        let _ = drain(&rx, Duration::from_millis(50));
        worker.stop();
    }
}
