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
use std::collections::VecDeque;
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
    /// 同 `run_js`，但把页面返回值原样带回来（不预先字符串化），
    /// `Coconut.toolbox.runJs` 用它拿真正的 JS 值。
    fn run_js_value(&self, plugin: &str, code: &str) -> Result<Value, String> {
        self.run_js(plugin, code)
            .map(|text| serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)))
    }
    /// 关掉工具箱页面，回到启动器首页。
    fn toolbox_exit(&self, _plugin: &str) -> Result<(), String> {
        Err("当前宿主不支持回到启动器".into())
    }
    /// 发起一次异步工具调用：立刻返回，结果稍后由宿主回送（`Plugins::tool_result`）。
    fn tool_call_begin(
        &self,
        _plugin: &str,
        _token: u64,
        _tool: &str,
        _args: &str,
    ) -> Result<(), String> {
        Err("当前宿主不支持异步工具调用".into())
    }
    /// 放弃一次异步工具调用（迟到的结果由 JS 侧丢弃）。
    fn tool_call_cancel(&self, _plugin: &str, _token: u64) -> Result<(), String> {
        Ok(())
    }
    /// 订阅 / 退订工具箱页面的 rAF（`Coconut.ui.requestAnimationFrame`）。
    /// 返回订阅者数量：0→1 时宿主负责让页面开始跑循环，1→0 时停掉。
    fn raf_subscribe(&self, _plugin: &str, _on: bool) -> Result<usize, String> {
        Err("当前宿主不支持 requestAnimationFrame".into())
    }
    /// 页面送来一帧：分发给订阅了 rAF 的插件。
    fn raf_tick(&self, _ts: f64) {}
    fn call_tool(&self, plugin: &str, tool: &str, args: &str) -> Result<String, String>;
    fn fetch(&self, plugin: &str, url: &str, opts: &str) -> Result<String, String>;
    /// 注册一个浮层，返回引擎分配的浮层 id（失败时返回错误文本）
    fn ui_overlay(&self, plugin: &str, html: &str, css: &str, script: &str) -> Result<u32, String>;
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
    /// 工具箱页面会话结束（关闭 / 重载 / 回到启动器）。
    Bye(Value),
    /// 一次异步工具调用的结果（`Coconut.tools.call`）。
    ToolResult(Value),
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

    pub fn bye(&self, info: Value) {
        let _ = self.tx.send(Job::Bye(info));
    }

    /// 异步工具调用的结果：直接投递给插件线程（会就地 settle 到 JS）。
    pub fn tool_result(&self, payload: Value) {
        let _ = self.tx.send(Job::ToolResult(payload));
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
    let mut deferred: VecDeque<Job> = VecDeque::new();
    report(
        &signals,
        run_hook(
            &context,
            &deadline,
            timeout,
            "onStart",
            &Value::Null,
            "",
            Some(&rx),
            &mut deferred,
        ),
    );

    loop {
        let job = if let Some(job) = deferred.pop_front() {
            Some(job)
        } else {
            match rx.recv_timeout(Duration::from_millis(8)) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = run_hook(
                        &context,
                        &deadline,
                        timeout,
                        "onStop",
                        &Value::Null,
                        "",
                        Some(&rx),
                        &mut deferred,
                    );
                    break;
                }
            }
        };

        let mut stop = false;
        if let Some(job) = job {
            // 异步工具调用的结果不需要跑钩子，直接 settle 到 JS 侧。
            if let Job::ToolResult(payload) = &job {
                settle_tool_result(&context, payload);
            } else {
                let (name, arg, event_type) = match &job {
                    Job::Hello(info) => ("onHello", info.clone(), String::new()),
                    Job::Bye(info) => ("onBye", info.clone(), String::new()),
                    Job::Event(event) => {
                        // 事件名由 payload 的 type 字段决定（debug 帧 → 'frame'）；
                        // 没有 type 的旧事件仍落到 'event'。
                        let kind = event
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        ("onEvent", event.clone(), kind)
                    }
                    Job::Stop => ("onStop", Value::Null, String::new()),
                    Job::ToolResult(_) => unreachable!(),
                };
                stop = name == "onStop";
                report(
                    &signals,
                    run_hook(
                        &context,
                        &deadline,
                        timeout,
                        name,
                        &arg,
                        &event_type,
                        Some(&rx),
                        &mut deferred,
                    ),
                );
            }
        }
        if stop {
            break;
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

        // runJsValue：同 runJs，但返回页面侧的真实值（Coconut.toolbox.runJs 用）
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "runJsValue",
                Function::new(ctx.clone(), move |code: String| -> String {
                    need!(perms, Permission::Dom, "dom");
                    match bridge.run_js_value(&id, &code) {
                        Ok(value) => ok_envelope(value),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // toolboxExit：回到启动器首页
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "toolboxExit",
                Function::new(ctx.clone(), move || -> String {
                    need!(perms, Permission::Toolbox, "toolbox");
                    match bridge.toolbox_exit(&id) {
                        Ok(()) => ok_envelope(Value::Null),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // rafSubscribe：Coconut.ui.requestAnimationFrame 的底层
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "rafSubscribe",
                Function::new(ctx.clone(), move |on: bool| -> String {
                    need!(perms, Permission::Ui, "ui");
                    match bridge.raf_subscribe(&id, on) {
                        Ok(count) => ok_envelope(json!(count)),
                        Err(error) => error_envelope(&error),
                    }
                })?,
            )?;
        }

        // toolBegin / toolCancel：Coconut.tools.call 的底层
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "toolBegin",
                Function::new(
                    ctx.clone(),
                    move |token: u64, tool: String, args: String| -> String {
                        need!(perms, Permission::Toolbox, "toolbox");
                        match bridge.tool_call_begin(&id, token, &tool, &args) {
                            Ok(()) => ok_envelope(Value::Null),
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
                "toolCancel",
                Function::new(ctx.clone(), move |token: u64| -> String {
                    need!(perms, Permission::Toolbox, "toolbox");
                    match bridge.tool_call_cancel(&id, token) {
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
        // 权限：overlay 只需要 ui（html/css 是数据，页面侧会净化）；
        //       带 script 时等于在页面里执行代码，额外要求 dom。
        {
            let bridge = bridge.clone();
            let id = plugin_id.to_string();
            let perms = permissions.to_vec();
            coconut.set(
                "uiOverlay",
                Function::new(
                    ctx.clone(),
                    move |html: String, css: String, script: String| -> String {
                        need!(perms, Permission::Ui, "ui");
                        if !script.trim().is_empty() {
                            need!(perms, Permission::Dom, "dom");
                        }
                        match bridge.ui_overlay(&id, &html, &css, &script) {
                            Ok(overlay) => json!({ "ok": true, "value": overlay }).to_string(),
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
///
/// `event_type` 是派发时给 `addEventListener` 用的事件名；空字符串表示按钩子名推导
/// （`onHello` → `hello`）。debug 帧会显式传 `frame`。
fn run_hook(
    context: &Context,
    deadline: &Arc<AtomicU64>,
    timeout: Duration,
    name: &str,
    arg: &Value,
    event_type: &str,
    rx: Option<&Receiver<Job>>,
    deferred: &mut VecDeque<Job>,
) -> Result<(), String> {
    let arg_json = arg.to_string();
    let limit = now_ms().saturating_add(timeout.as_millis() as u64);
    deadline.store(limit, Ordering::SeqCst);

    let started = context.with(|ctx| -> Result<bool, rquickjs::Error> {
        ctx.eval::<(), _>(format!("__coconut_hook_start({})", json!(name)))?;
        let invoke = ctx.globals().get::<_, Function>("__coconut_hook_invoke")?;
        invoke.call::<_, bool>((arg_json, event_type.to_string()))
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
            // 钩子里可能在等一次异步工具调用的结果（`await Coconut.tools.call(...)`）：
            // 把已经到达的 Job 就地消化掉，否则会被判成「挂起」。
            if let Some(rx) = rx {
                while let Ok(job) = rx.try_recv() {
                    match job {
                        Job::ToolResult(payload) => settle_tool_result(context, &payload),
                        // 其它 Job 先存着，等这一轮钩子跑完再按顺序处理
                        other => deferred.push_back(other),
                    }
                }
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

/// 把一次异步工具调用的结果交给 JS 侧（解析 promise + 派发 result / error 事件）。
fn settle_tool_result(context: &Context, payload: &Value) {
    let text = payload.to_string();
    let _ = context.with(|ctx| -> Result<(), rquickjs::Error> {
        if let Ok(settle) = ctx.globals().get::<_, Function>("__coconut_tool_settle") {
            let _ = settle.call::<_, u32>((text,));
        }
        Ok(())
    });
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

  // ── 事件核心：DOMException / Event / EventTarget / AbortController ──────
  // 全部向 web 标准靠：dispatch 同步、单参数事件对象、{ once, signal } 选项。
  // 刻意不实现 capture / passive / 冒泡：这里没有 DOM 树，冒泡没有意义。
  class DOMException extends Error {
    constructor(message, name) {
      super(message === undefined ? '' : String(message));
      this.name = name === undefined ? 'Error' : String(name);
    }
  }

  class CoconutEvent {
    constructor(type, init) {
      const options = init === undefined || init === null ? {} : init;
      this.type = String(type === undefined ? '' : type);
      this.detail = options.detail === undefined ? null : options.detail;
      this.target = null;
      this.currentTarget = null;
      this.timeStamp = Date.now();
      this.defaultPrevented = false;
      this.cancelable = options.cancelable === true;
      // 内部用：监听器返回的 promise / 同步抛出的错误
      this.__pending = [];
      this.__errors = [];
    }

    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    }
  }

  const eventListeners = new WeakMap();

  function listenersMap(target) {
    let map = eventListeners.get(target);
    if (!map) {
      map = new Map();
      eventListeners.set(target, map);
    }
    return map;
  }

  class EventTarget {
    addEventListener(type, listener, options) {
      if (listener === null || listener === undefined) return;
      if (typeof listener !== 'function') {
        throw new TypeError('listener 必须是函数');
      }
      const key = String(type);
      const opts = typeof options === 'object' && options !== null ? options : {};
      if (opts.capture === true || opts.passive === true) {
        // 平台没有捕获/冒泡的概念，静默忽略最坑人，这里明确提示一次
        try {
          console.warn('[coconut] addEventListener: capture / passive 不支持，已忽略');
        } catch (ignored) {}
      }
      if (opts.signal && opts.signal.aborted) return;
      const map = listenersMap(this);
      let list = map.get(key);
      if (!list) {
        list = [];
        map.set(key, list);
      }
      for (let i = 0; i < list.length; i++) {
        if (list[i].fn === listener) return; // 重复注册忽略，和 DOM 一致
      }
      const entry = {
        fn: listener,
        once: opts.once === true,
        signal: opts.signal || null,
        abortHandler: null
      };
      list.push(entry);
      if (entry.signal) {
        const target = this;
        entry.abortHandler = function () {
          target.removeEventListener(key, listener);
        };
        entry.signal.addEventListener('abort', entry.abortHandler, { once: true });
      }
    }

    removeEventListener(type, listener) {
      const map = listenersMap(this);
      const list = map.get(String(type));
      if (!list) return;
      for (let i = 0; i < list.length; i++) {
        if (list[i].fn !== listener) continue;
        const entry = list[i];
        if (entry.signal && entry.abortHandler) {
          entry.signal.removeEventListener('abort', entry.abortHandler);
        }
        list.splice(i, 1);
        break;
      }
    }

    dispatchEvent(event) {
      if (!(event instanceof CoconutEvent)) {
        throw new TypeError('dispatchEvent 需要 Coconut.Event 实例');
      }
      event.target = this;
      event.currentTarget = this;
      const list = (listenersMap(this).get(event.type) || []).slice();
      for (let i = 0; i < list.length; i++) {
        const entry = list[i];
        if (entry.once) this.removeEventListener(event.type, entry.fn);
        try {
          const result = entry.fn.call(this, event);
          if (result && typeof result.then === 'function') event.__pending.push(result);
        } catch (thrown) {
          event.__errors.push(thrown);
          try {
            console.error('[coconut] 事件监听器抛错：' + String((thrown && thrown.message) || thrown));
          } catch (ignored) {}
        }
      }
      event.currentTarget = null;
      return !event.defaultPrevented;
    }

    /** 内部用：钩子兼容层（返回监听器函数数组） */
    __listeners(type) {
      const list = listenersMap(this).get(String(type)) || [];
      return list.map(function (entry) { return entry.fn; });
    }
  }

  class AbortSignal extends EventTarget {
    constructor() {
      super();
      this.aborted = false;
      this.reason = undefined;
    }

    throwIfAborted() {
      if (this.aborted) throw this.reason;
    }

    static timeout(ms) {
      const controller = new AbortController();
      const delay = Number(ms);
      globalThis.setTimeout(function () {
        controller.abort(new DOMException('信号超时', 'TimeoutError'));
      }, isFinite(delay) && delay > 0 ? delay : 0);
      return controller.signal;
    }

    static abort(reason) {
      const controller = new AbortController();
      controller.abort(reason);
      return controller.signal;
    }
  }

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }

    abort(reason) {
      const signal = this.signal;
      if (signal.aborted) return;
      signal.aborted = true;
      signal.reason = reason === undefined
        ? new DOMException('信号已中止', 'AbortError')
        : reason;
      signal.dispatchEvent(new CoconutEvent('abort'));
    }
  }

  /** http / webSocket 这些内部事件源统一用 EventTarget，插件也能自己 new 一个。 */
  function createEmitter() {
    return new EventTarget();
  }

  function emit(target, type, detail, cancelable) {
    target.dispatchEvent(
      new CoconutEvent(type, {
        detail: detail === undefined ? null : detail,
        cancelable: cancelable === true
      })
    );
  }

  // ── JSX 片段协议 ────────────────────────────────────────────────────────
  // SDK 的 JSX 运行时会把裸标签变成 { __coconut: 'script'|'html'|'css', text }：
  //   <script>…</script> → Coconut.toolbox.runJs(...)
  //   <html>…</html>     → Coconut.ui.overlay({ html })
  //   <style>…</style>   → Coconut.ui.overlay({ css })
  function chunkOf(value) {
    if (!value || typeof value !== 'object') return null;
    const kind = value.__coconut;
    if (kind !== 'script' && kind !== 'html' && kind !== 'css') return null;
    return { kind: kind, text: String(value.text === undefined ? '' : value.text) };
  }

  function chunkText(value, kind) {
    const chunk = chunkOf(value);
    if (!chunk) {
      return value === undefined || value === null ? '' : String(value);
    }
    if (chunk.kind !== kind) {
      throw new TypeError('这里要的是 <' + kind + '> 片段，实际收到 <' + chunk.kind + '>');
    }
    return chunk.text;
  }

  // 已弃用的老 API：保留可调用，但第一次用会警告一次（不重复刷屏）
  const deprecatedHits = Object.create(null);
  function deprecated(name, replacement) {
    if (deprecatedHits[name]) return;
    deprecatedHits[name] = true;
    try {
      console.warn('[coconut] Coconut.' + name + ' 已 @deprecated，请改用 ' + replacement);
    } catch (ignored) {}
  }

  const Coconut = createEmitter();
  Object.assign(Coconut, {
    plugin: meta,
    runJs: function (code) {
      deprecated('runJs', 'Coconut.toolbox.runJs');
      return String(unwrap(coconut.runJs(String(code))));
    },
    callTool: function (tool, args) {
      deprecated('callTool', 'Coconut.tools.call');
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
        if (typeof opts === 'string') opts = { html: opts };
        var html = chunkText(opts.html, 'html');
        var css = chunkText(opts.css, 'css');
        // script：页面侧执行的函数（需要 dom）。传函数对象或 <script> 片段都行。
        var script = '';
        if (typeof opts.script === 'function') {
          script = Function.prototype.toString.call(opts.script);
        } else if (opts.script !== undefined && opts.script !== null) {
          script = chunkText(opts.script, 'script');
        }
        return unwrap(coconut.uiOverlay(String(html), String(css), String(script)));
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
    /** 工具箱本身：页面脚本执行、回到启动器。 */
    toolbox: {
      /**
       * 在工具箱页面里执行代码，返回页面侧的结果（已反序列化成 JS 值）。
       *
       *   1) 传字符串：当作函数体，必须自己 return：
       *        Coconut.toolbox.runJs('return document.title')
       *   2) 传函数对象：引擎取 Function.prototype.toString() 送到页面执行，
       *      后面的参数会被 JSON 序列化后传进去：
       *        Coconut.toolbox.runJs(function (name) {
       *          return document.querySelector('[data-sf="sidebar"]').textContent + name
       *        }, '！')
       *
       * ⚠️ 函数必须是**自包含**的：闭包里捕获的插件变量在页面里不存在，
       *    要用的数据请走参数。返回值必须是能 JSON 序列化的东西；
       *    async 函数跨不过进程边界（页面侧拿不到 Promise），会直接抛 TypeError。
       */
      runJs: function (code) {
        const args = Array.prototype.slice.call(arguments, 1);
        const packed = JSON.stringify(args);
        if (typeof code === 'function') {
          if (code.constructor && code.constructor.name === 'AsyncFunction') {
            throw new TypeError(
              'Coconut.toolbox.runJs 不支持 async 函数：页面侧拿不到 Promise，请写成同步函数'
            );
          }
          const source = Function.prototype.toString.call(code);
          if (source.indexOf('[native code]') >= 0) {
            throw new TypeError('Coconut.toolbox.runJs 不能执行原生 / 绑定函数');
          }
          return unwrap(
            coconut.runJsValue('return (' + source + ').apply(undefined, ' + packed + ');')
          );
        }
        if (chunkOf(code)) {
          code = chunkText(code, 'script');
        }
        if (typeof code !== 'string') {
          throw new TypeError('Coconut.toolbox.runJs 需要字符串、函数或 <script> 片段');
        }
        if (!args.length) return unwrap(coconut.runJsValue(code));
        return unwrap(coconut.runJsValue('return (' + code + ').apply(undefined, ' + packed + ');'));
      },
      /** 关掉工具箱页面，回到启动器首页（插件侧等同于「退出工具箱」）。 */
      exit: function () {
        unwrap(coconut.toolboxExit());
        return true;
      }
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

  /** socket 事件：detail 里放一份，同时按 WebSocket 标准把字段挂到事件本身。 */
  function emitSocket(target, type, detail) {
    const event = new CoconutEvent(type, { detail: detail });
    for (const key in detail) {
      if (Object.prototype.hasOwnProperty.call(detail, key)) event[key] = detail[key];
    }
    target.dispatchEvent(event);
  }

  function dispatchRealtime(event) {
    const type = String(event.type || '');
    if (type === 'http-request') {
      dispatchHttp(event);
      return;
    }
    if (event.source === 'raf') {
      globalThis.__coconut_raf_tick(Number(event.ts || 0));
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
        emitSocket(socket, 'message', { data: String(event.data || '') });
      } else if (type === 'ws-close') {
        emitSocket(socket, 'close', {
          code: Number(event.code || 1005),
          reason: String(event.reason || ''),
          wasClean: event.clean !== false
        });
        serverSockets.delete(String(event.server) + ':' + String(event.conn));
      }
      return;
    }
    if (event.source === 'client') {
      const socket = webSockets.get(Number(event.client));
      if (!socket) return;
      if (type === 'ws-open') {
        socket.connected = true;
        socket.readyState = WS_OPEN;
        emitSocket(socket, 'open', {});
      } else if (type === 'ws-message') {
        emitSocket(socket, 'message', { data: String(event.data || '') });
      } else if (type === 'ws-close') {
        socket.connected = false;
        socket.readyState = WS_CLOSED;
        emitSocket(socket, 'close', {
          code: Number(event.code || 1005),
          reason: String(event.reason || ''),
          wasClean: event.clean !== false
        });
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

  // WebSocket 状态常量（和浏览器一致）
  const WS_CONNECTING = 0;
  const WS_OPEN = 1;
  const WS_CLOSING = 2;
  const WS_CLOSED = 3;

  Coconut.webSocket = function (url) {
    if (typeof url !== 'string' || !url) throw new TypeError('webSocket(url) 需要一个地址');
    // 宿主是同步连接：wsConnect 返回时已经连上了，所以初始状态就是 OPEN。
    const info = unwrap(coconut.wsConnect(url));
    const id = Number(info.id);
    const socket = createEmitter();
    socket.id = id;
    socket.url = url;
    socket.connected = true;
    socket.readyState = WS_OPEN;
    socket.CONNECTING = WS_CONNECTING;
    socket.OPEN = WS_OPEN;
    socket.CLOSING = WS_CLOSING;
    socket.CLOSED = WS_CLOSED;
    socket.send = function (data) {
      if (socket.readyState !== WS_OPEN) {
        throw new DOMException('WebSocket 不在 OPEN 状态，不能发送', 'InvalidStateError');
      }
      return Boolean(unwrap(coconut.wsSend(id, String(data))));
    };
    socket.close = function (code, reason) {
      if (socket.readyState === WS_CLOSING || socket.readyState === WS_CLOSED) return true;
      socket.readyState = WS_CLOSING;
      try {
        return Boolean(unwrap(coconut.wsClose(id)));
      } finally {
        socket.connected = false;
        socket.readyState = WS_CLOSED;
        webSockets.delete(id);
        // 本地先派发一次 close（宿主那边的 ws-close 事件会因为没有这个 socket 而被忽略）
        emitSocket(socket, 'close', {
          code: Number(code) || 1005,
          reason: String(reason === undefined ? '' : reason),
          wasClean: true
        });
      }
    };
    webSockets.set(id, socket);
    return socket;
  };

  Coconut.webSocket.CONNECTING = WS_CONNECTING;
  Coconut.webSocket.OPEN = WS_OPEN;
  Coconut.webSocket.CLOSING = WS_CLOSING;
  Coconut.webSocket.CLOSED = WS_CLOSED;

  globalThis.Coconut = Coconut;

  // ── Coconut.tools：异步 + 事件版的工具调用 ──────────────────────────────
  //   tools.call(tool, args, { signal }) → Promise
  //   事件：'result' / 'error'（detail 里有 { id, tool, args, result?, error?, aborted?, ms }）
  let toolToken = 0;
  const toolPending = new Map();

  Coconut.tools = createEmitter();

  Coconut.tools.call = function (tool, args, options) {
    const name = String(tool);
    const payload = args === undefined || args === null ? {} : args;
    const opts = options === undefined || options === null ? {} : options;
    const signal = opts.signal || null;
    if (signal && signal.aborted) {
      return Promise.reject(signal.reason || new DOMException('调用已中止', 'AbortError'));
    }
    return new Promise(function (resolve, reject) {
      const token = ++toolToken;
      const entry = {
        token: token,
        tool: name,
        args: payload,
        started: Date.now(),
        resolve: resolve,
        reject: reject,
        signal: signal,
        onAbort: null
      };
      toolPending.set(token, entry);
      if (signal) {
        entry.onAbort = function () {
          if (!toolPending.has(token)) return;
          toolPending.delete(token);
          try {
            coconut.toolCancel(token);
          } catch (ignored) {}
          reject(signal.reason || new DOMException('调用已中止', 'AbortError'));
        };
        signal.addEventListener('abort', entry.onAbort, { once: true });
      }
      try {
        unwrap(coconut.toolBegin(token, name, JSON.stringify(payload)));
      } catch (error) {
        toolPending.delete(token);
        if (signal && entry.onAbort) signal.removeEventListener('abort', entry.onAbort);
        reject(error);
      }
    });
  };

  // ── Coconut.ui.requestAnimationFrame ────────────────────────────────────
  // 由工具箱页面的 rAF 合并后回传（默认 ≤30Hz），**不是** 60fps 实时循环：
  // 插件线程是 8ms 轮询 + WS 往返。需要丝滑动画就把脚本写进页面（overlay 的 script）。
  let rafSeq = 0;
  let rafSubscribed = false;
  const rafCallbacks = new Map();

  function ensureRaf() {
    if (rafSubscribed) return;
    rafSubscribed = true;
    try {
      unwrap(coconut.rafSubscribe(true));
    } catch (error) {
      rafSubscribed = false;
      throw error;
    }
  }

  function dropRaf() {
    if (!rafSubscribed) return;
    rafSubscribed = false;
    try {
      unwrap(coconut.rafSubscribe(false));
    } catch (ignored) {}
  }

  Coconut.ui.requestAnimationFrame = function (callback) {
    if (typeof callback !== 'function') {
      throw new TypeError('requestAnimationFrame 需要一个函数');
    }
    const id = ++rafSeq;
    rafCallbacks.set(id, callback);
    ensureRaf();
    return id;
  };

  Coconut.ui.cancelAnimationFrame = function (id) {
    rafCallbacks.delete(Number(id));
    if (!rafCallbacks.size) dropRaf();
  };

  /** 页面送来一帧：按注册顺序回调，参数是页面时间戳（毫秒）。 */
  globalThis.__coconut_raf_tick = function (ts) {
    const entries = Array.from(rafCallbacks.entries());
    for (let i = 0; i < entries.length; i++) {
      const id = entries[i][0];
      const fn = entries[i][1];
      if (!rafCallbacks.has(id)) continue;
      try {
        fn.call(undefined, Number(ts));
      } catch (error) {
        safeReport(error);
      }
    }
    return entries.length;
  };

  // 页面没了：订阅作废，回调留给下一次 onHello 重新注册。
  Coconut.addEventListener('bye', function () {
    rafCallbacks.clear();
    rafSubscribed = false;
  });

  /** host 侧工具调用结束时回调：先派发事件，再 settle promise。 */
  globalThis.__coconut_tool_settle = function (payloadJson) {
    let payload = null;
    try {
      payload = JSON.parse(String(payloadJson));
    } catch (ignored) {
      return 0;
    }
    if (!payload || typeof payload !== 'object') return 0;
    const token = Number(payload.token);
    const entry = toolPending.get(token);
    if (!entry) return 0; // 已取消 / 已 settle：丢掉迟到的结果
    toolPending.delete(token);
    if (entry.signal && entry.onAbort) {
      entry.signal.removeEventListener('abort', entry.onAbort);
    }
    const detail = {
      id: token,
      tool: entry.tool,
      args: entry.args,
      ms: Date.now() - entry.started
    };
    if (payload.ok) {
      detail.result = payload.result;
      emit(Coconut.tools, 'result', detail);
      entry.resolve(payload.result);
    } else {
      detail.error = String(payload.error || '工具调用失败');
      if (payload.aborted) detail.aborted = true;
      emit(Coconut.tools, 'error', detail);
      entry.reject(
        payload.aborted
          ? new DOMException(detail.error, 'AbortError')
          : new Error(detail.error)
      );
    }
    return 1;
  };

  // 事件 / 中止相关的标准名：既挂 Coconut 上，也直接挂全局，
  // 插件里可以写 new Coconut.Event(...) 或 new AbortController()。
  Coconut.Event = CoconutEvent;
  Coconut.EventTarget = EventTarget;
  Coconut.AbortController = AbortController;
  Coconut.AbortSignal = AbortSignal;
  Coconut.DOMException = DOMException;
  globalThis.Event = CoconutEvent;
  globalThis.EventTarget = EventTarget;
  globalThis.AbortController = AbortController;
  globalThis.AbortSignal = AbortSignal;
  globalThis.DOMException = DOMException;

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

  globalThis.__coconut_hook_invoke = function (argJson, typeOverride) {
    const name = String(current);
    const fn = globalThis[name];
    // 事件名由调用点显式给出（debug 帧是 'frame'）；没给就按钩子名推导。
    const eventType = typeOverride && String(typeOverride)
      ? String(typeOverride)
      : name.replace(/^on/, '').toLowerCase();
    const listeners = Coconut.__listeners(eventType);
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
    let arg = null;
    try {
      arg = argJson ? JSON.parse(argJson) : null;
    } catch (thrown) {
      finish(thrown);
      return false;
    }
    // 生命周期也走事件：事件名就是 addEventListener 能订阅的名字。
    const event = new CoconutEvent(eventType, { detail: arg });
    const jobs = [];
    try {
      // 1) 全局钩子（等价于最先注册的监听器）：保留 onHello / onEvent / onStop 写法
      if (typeof fn === 'function') {
        const result = fn.call(globalThis, arg);
        if (result && typeof result.then === 'function') jobs.push(result);
      }
      // 2) 事件监听器：同步派发，异步结果收进 event.__pending
      Coconut.dispatchEvent(event);
      for (let i = 0; i < event.__pending.length; i++) jobs.push(event.__pending[i]);
      // 钩子路径下监听器同步抛错仍然算这次钩子失败（socket/http 事件路径只记日志）
      if (event.__errors.length) {
        finish(event.__errors[0]);
        return false;
      }
      if (jobs.length) {
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
        codes: Mutex<Vec<String>>,
        raf: Mutex<Vec<bool>>,
    }

    impl Recorder {
        fn logs(&self) -> Vec<String> {
            self.logs.lock().unwrap().clone()
        }
        fn raf_calls(&self) -> Vec<bool> {
            self.raf.lock().unwrap().clone()
        }
        fn codes(&self) -> Vec<String> {
            self.codes.lock().unwrap().clone()
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
        fn run_js(&self, _plugin: &str, code: &str) -> Result<String, String> {
            self.codes.lock().unwrap().push(code.to_string());
            Ok("{\"ok\":1}".to_string())
        }
        fn run_js_value(&self, _plugin: &str, code: &str) -> Result<Value, String> {
            self.codes.lock().unwrap().push(code.to_string());
            Ok(Value::from(1))
        }
        fn call_tool(&self, _plugin: &str, tool: &str, args: &str) -> Result<String, String> {
            self.tools.lock().unwrap().push(format!("{tool} {args}"));
            Ok("{\"echo\":true}".to_string())
        }
        fn fetch(&self, _plugin: &str, url: &str, _opts: &str) -> Result<String, String> {            self.fetches.lock().unwrap().push(url.to_string());
            Ok("{\"data\":[]}".to_string())
        }
        fn ui_overlay(&self, _plugin: &str, html: &str, css: &str, script: &str) -> Result<u32, String> {
            let mut panels = self.panels.lock().unwrap();
            panels.push(format!("{html} {css} {script}"));
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

        fn tool_call_begin(
            &self,
            _plugin: &str,
            token: u64,
            tool: &str,
            args: &str,
        ) -> Result<(), String> {
            self.tools
                .lock()
                .unwrap()
                .push(format!("async {token} {tool} {args}"));
            Ok(())
        }

        fn raf_subscribe(&self, _plugin: &str, on: bool) -> Result<usize, String> {
            self.raf.lock().unwrap().push(on);
            Ok(if on { 1 } else { 0 })
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

    #[test]
    fn tools_call_resolves_from_a_tool_result_job() {
        let source = r#"
        function onStart() {
          Coconut.tools.addEventListener('result', function (event) {
            Coconut.log('evt', event.detail.tool, JSON.stringify(event.detail.result));
          });
          Coconut.tools.call('get_result_summary', { a: 1 }).then(
            function (value) { Coconut.log('resolved', JSON.stringify(value)); },
            function (error) { Coconut.log('failed', String(error.message)); }
          );
          Coconut.log('started');
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Toolbox]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        while std::time::Instant::now() < deadline && recorder.tools().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            recorder.tools(),
            vec!["async 1 get_result_summary {\"a\":1}".to_string()],
            "tools: {:?}",
            recorder.tools()
        );
        worker.tool_result(json!({
            "token": 1,
            "ok": true,
            "result": { "b": 2 }
        }));
        let _ = drain(&rx, Duration::from_millis(400));
        let logs = recorder.logs();
        worker.stop();
        assert!(
            logs.iter().any(|line| line == "evt get_result_summary {\"b\":2}"),
            "logs: {logs:?}"
        );
        assert!(
            logs.iter().any(|line| line == "resolved {\"b\":2}"),
            "logs: {logs:?}"
        );
    }

    #[test]
    fn tools_call_can_be_awaited_inside_a_hook() {
        let source = r#"
        async function onStart() {
          Coconut.log('started');
          try {
            const value = await Coconut.tools.call('get_result_summary', {});
            Coconut.log('awaited', JSON.stringify(value));
          } catch (error) {
            Coconut.log('failed', String(error.message));
          }
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Toolbox]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        while std::time::Instant::now() < deadline && recorder.tools().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(recorder.tools().len(), 1, "tools: {:?}", recorder.tools());
        worker.tool_result(json!({ "token": 1, "ok": true, "result": 7 }));
        let _ = drain(&rx, Duration::from_millis(500));
        let logs = recorder.logs();
        worker.stop();
        assert!(
            logs.iter().any(|line| line == "awaited 7"),
            "钩子里的 await 没被泵起来: {logs:?}"
        );
    }

    #[test]
    fn overlay_needs_ui_but_script_also_needs_dom() {
        let source = r#"
        function onStart() {
          const id = Coconut.ui.overlay({ html: '<b>plain</b>' });
          Coconut.log('plain', String(id));
          try {
            Coconut.ui.overlay({ html: '<b>x</b>', script: function (root) { return null; } });
            Coconut.log('script-allowed');
          } catch (error) {
            Coconut.log('script-denied', String(error.message));
          }
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Ui]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(600);
        while std::time::Instant::now() < deadline && recorder.logs().len() < 2 {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = drain(&rx, Duration::from_millis(200));
        let logs = recorder.logs();
        let panels = recorder.panels();
        worker.stop();
        assert!(
            logs.iter().any(|line| line == "plain 1"),
            "只有 ui 权限时普通浮层应该可用: {logs:?}"
        );
        assert!(
            logs.iter().any(|line| line.starts_with("script-denied") && line.contains("dom")),
            "带 script 时应该因缺 dom 权限被拒: {logs:?}"
        );
        assert_eq!(panels.len(), 1, "被拒的那次不该创建浮层: {panels:?}");
    }

    #[test]
    fn request_animation_frame_runs_on_page_ticks() {
        let source = r#"
        let ticks = 0;
        function onStart() {
          const id = Coconut.ui.requestAnimationFrame(function (ts) {
            ticks += 1;
            Coconut.log('tick', String(ts));
            if (ticks >= 2) Coconut.ui.cancelAnimationFrame(id);
          });
          Coconut.log('scheduled', String(id));
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Ui]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while std::time::Instant::now() < deadline && recorder.logs().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        for ts in [16.5, 33.0] {
            recorder
                .events()
                .lock()
                .unwrap()
                .push(json!({ "type": "raf", "source": "raf", "ts": ts }));
            std::thread::sleep(Duration::from_millis(120));
        }
        let _ = drain(&rx, Duration::from_millis(300));
        let logs = recorder.logs();
        let calls = recorder.raf_calls();
        worker.stop();
        assert!(
            logs.iter().any(|line| line == "scheduled 1"),
            "logs: {logs:?}"
        );
        assert_eq!(
            logs.iter().filter(|line| line.starts_with("tick ")).count(),
            2,
            "logs: {logs:?}"
        );
        assert_eq!(calls, vec![true, false], "订阅 / 退订应该各一次: {calls:?}");
    }

    #[test]
    fn event_target_supports_once_and_abort_signal() {
        let source = r#"
        function onStart() {
          const bus = new Coconut.EventTarget();
          let hits = 0;
          bus.addEventListener('tick', function (event) {
            hits += 1;
            Coconut.log('tick', String(event.type), String(event.detail.n));
          });
          const once = new Coconut.EventTarget();
          once.addEventListener('only', function () { Coconut.log('once'); }, { once: true });
          const controller = new AbortController();
          const cancelled = new Coconut.EventTarget();
          cancelled.addEventListener('gone', function () { Coconut.log('should-not-run'); }, {
            signal: controller.signal
          });
          controller.abort();

          bus.dispatchEvent(new Coconut.Event('tick', { detail: { n: 1 } }));
          bus.dispatchEvent(new Coconut.Event('tick', { detail: { n: 2 } }));
          once.dispatchEvent(new Coconut.Event('only'));
          once.dispatchEvent(new Coconut.Event('only'));
          cancelled.dispatchEvent(new Coconut.Event('gone'));
          Coconut.log('hits', String(hits), String(controller.signal.aborted));

          const caught = new Coconut.EventTarget();
          caught.addEventListener('boom', function () { throw new Error('listener-boom'); });
          const result = caught.dispatchEvent(new Coconut.Event('boom'));
          Coconut.log('dispatch-ok', String(result));
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
        let (_, errors) = drain(&rx, Duration::from_millis(300));
        let logs = recorder.logs();
        worker.stop();
        assert!(errors.is_empty(), "errors: {errors:?}; logs: {logs:?}");
        assert_eq!(
            logs.iter().filter(|line| *line == "tick tick 1").count(),
            1,
            "logs: {logs:?}"
        );
        assert_eq!(
            logs.iter().filter(|line| *line == "once").count(),
            1,
            "once 没生效: {logs:?}"
        );
        assert!(
            !logs.iter().any(|line| line == "should-not-run"),
            "signal 注销没生效: {logs:?}"
        );
        assert!(logs.iter().any(|line| line == "hits 2 true"), "logs: {logs:?}");
        assert!(
            logs.iter().any(|line| line == "dispatch-ok true"),
            "监听器抛错不该打断派发: {logs:?}"
        );
    }

    #[test]
    fn jsx_chunks_are_accepted_by_run_js_and_overlay() {
        let source = r#"
        function onStart() {
          // SDK 的 JSX 运行时会产出这种片段对象
          const script = { __coconut: 'script', text: 'return 6 * 7' };
          const html = { __coconut: 'html', text: '<b class="x">X</b>' };
          const css = { __coconut: 'css', text: '.x{color:#fff}' };
          Coconut.toolbox.runJs(script);
          const id = Coconut.ui.overlay({ html: html, css: css });
          Coconut.log('overlay', String(id));
          try {
            Coconut.ui.overlay({ html: script });
            Coconut.log('bad-kind');
          } catch (error) {
            Coconut.log('kind-guard', String(error.name));
          }
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Dom, Permission::Ui]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let deadline = std::time::Instant::now() + Duration::from_millis(800);
        while std::time::Instant::now() < deadline && recorder.panels().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = drain(&rx, Duration::from_millis(200));
        let codes = recorder.codes();
        let panels = recorder.panels();
        let logs = recorder.logs();
        worker.stop();
        assert!(
            codes.iter().any(|code| code == "return 6 * 7"),
            "codes: {codes:?}"
        );
        assert!(
            panels.first().map(|panel| panel.starts_with("<b class=\"x\">X</b>")) == Some(true),
            "panels: {panels:?}"
        );
        assert!(
            logs.iter().any(|line| line == "kind-guard TypeError"),
            "logs: {logs:?}"
        );
    }

    #[test]
    fn toolbox_run_js_accepts_function_objects() {
        let source = r#"
        function onStart() {
          const result = Coconut.toolbox.runJs(function (a, b) {
            return a + b;
          }, 41, 'x');
          Coconut.log('result', String(result));
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Dom]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let (_, errors) = drain(&rx, Duration::from_millis(300));
        let codes = recorder.codes();
        let logs = recorder.logs();
        worker.stop();
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(codes.len(), 1, "codes: {codes:?}");
        assert!(
            codes[0].contains("a + b"),
            "函数源码没送到页面: {codes:?}"
        );
        assert!(
            codes[0].contains("[41,\"x\"]"),
            "参数没序列化进去: {codes:?}"
        );
        assert!(logs.iter().any(|line| line == "result 1"), "logs: {logs:?}");
    }

    #[test]
    fn toolbox_run_js_rejects_async_functions() {
        let source = r#"
        function onStart() {
          try {
            Coconut.toolbox.runJs(async function () { return 1; });
            Coconut.log('no-throw');
          } catch (error) {
            Coconut.log('rejected', String(error.name));
          }
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Dom]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let _ = drain(&rx, Duration::from_millis(300));
        let logs = recorder.logs();
        worker.stop();
        assert!(logs.iter().any(|line| line == "rejected TypeError"), "logs: {logs:?}");
    }

    #[test]
    fn bye_reaches_the_onbye_hook() {
        let source = r#"
        function onBye(info) {
          Coconut.log('bye', String(info && info.session));
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
        let _ = drain(&rx, Duration::from_millis(100));
        worker.bye(json!({ "session": "s9" }));
        let _ = drain(&rx, Duration::from_millis(300));
        let logs = recorder.logs();
        worker.stop();
        assert!(logs.iter().any(|line| line == "bye s9"), "logs: {logs:?}");
    }

    #[test]
    fn frame_events_are_dispatched_under_their_own_type() {
        let source = r#"
        function onStart() {
          Coconut.addEventListener('frame', function (event) {
            Coconut.log('frame-listener', String(event.detail.direction));
          });
        }
        "#;
        let recorder = Arc::new(Recorder::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut worker = spawn(
            plugin(source, &[Permission::Debug]),
            recorder.clone(),
            tx,
            Duration::from_secs(5),
        );
        let _ = drain(&rx, Duration::from_millis(100));
        worker.event(json!({
            "type": "frame",
            "direction": "recv",
            "frame": { "type": "hello" },
            "at": 1
        }));
        let _ = drain(&rx, Duration::from_millis(300));
        let logs = recorder.logs();
        worker.stop();
        assert!(
            logs.iter().any(|line| line == "frame-listener recv"),
            "logs: {logs:?}"
        );
    }
}
