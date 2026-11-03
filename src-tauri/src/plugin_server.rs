//! 插件自己的 HTTP / WebSocket 服务，以及 WebSocket 客户端。
//!
//! 这些能力不在插件线程里跑：axum / tokio-tungstenite 的任务都挂在
//! [`crate::server::bridge_runtime`] 上。插件线程每 8ms 通过
//! `coconut.pollEvents()` 取事件，再在 QuickJS 里回调插件的监听器；
//! 响应 / 发送则通过同步宿主函数回到这里。
//!
//! 之所以拆成独立模块：`server.rs` 已经有工具箱静态服务、游戏数据、工坊反代等
//! 一大套逻辑，插件服务是另一条生命周期（随插件启停），混在一起会很难看清边界。

use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        FromRequestParts, Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

const MAX_BODY: usize = 32 * 1024 * 1024;
/// 插件 handler 迟迟不响应时，HTTP 请求最多等这么久（模型流可能很慢）。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub struct PluginServers {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    next_server: AtomicU64,
    next_request: AtomicU64,
    next_connection: AtomicU64,
    next_client: AtomicU64,
    servers: Mutex<HashMap<u64, ServerEntry>>,
    responses: Mutex<HashMap<u64, RequestHandle>>,
    sockets: Mutex<HashMap<(u64, u64), SocketHandle>>,
    clients: Mutex<HashMap<u64, ClientHandle>>,
    events: Mutex<Vec<(String, Value)>>,
}

struct ServerEntry {
    plugin: String,
    shutdown: Option<oneshot::Sender<()>>,
}

struct RequestHandle {
    plugin: String,
    tx: mpsc::UnboundedSender<ResponseAction>,
}

struct SocketHandle {
    plugin: String,
    tx: mpsc::UnboundedSender<SocketCommand>,
}

struct ClientHandle {
    plugin: String,
    tx: mpsc::UnboundedSender<SocketCommand>,
}

enum ResponseAction {
    Chunk {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        end: bool,
    },
}

enum SocketCommand {
    Text(String),
    Close,
}

#[derive(Clone)]
struct AppCtx {
    servers: PluginServers,
    server: u64,
}

impl PluginServers {
    pub fn new() -> Self {
        Self::default()
    }

    fn push_event(&self, plugin: &str, event: Value) {
        if let Ok(mut events) = self.inner.events.lock() {
            events.push((plugin.to_string(), event));
        }
    }

    fn next_server(&self) -> u64 {
        self.inner.next_server.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn next_request(&self) -> u64 {
        self.inner.next_request.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn next_connection(&self) -> u64 {
        self.inner.next_connection.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn next_client(&self) -> u64 {
        self.inner.next_client.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 开一个同时支持 HTTP 与 WebSocket 的本地服务。端口传 0 由系统分配。
    pub fn listen(&self, plugin: &str, host: &str, port: u16) -> Result<Value, String> {
        let host = if host.trim().is_empty() {
            "127.0.0.1"
        } else {
            host.trim()
        };
        if host.contains('/') || host.contains('\\') {
            return Err("server host 不合法".into());
        }
        let address = format!("{host}:{port}");
        let listener = crate::server::bridge_block_on(|| TcpListener::bind(address.clone()))
            .map_err(|error| format!("监听 {address} 失败：{error}"))?;
        let actual_port = listener
            .local_addr()
            .map_err(|error| format!("读取监听端口失败：{error}"))?
            .port();
        let id = self.next_server();
        let (shutdown, shutdown_rx) = oneshot::channel();
        let app = Router::new().fallback(handle_entry).with_state(AppCtx {
            servers: self.clone(),
            server: id,
        });
        crate::server::bridge_runtime().spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        if let Ok(mut servers) = self.inner.servers.lock() {
            servers.insert(
                id,
                ServerEntry {
                    plugin: plugin.to_string(),
                    shutdown: Some(shutdown),
                },
            );
        }
        Ok(json!({ "id": id, "port": actual_port }))
    }

    pub fn close(&self, plugin: &str, id: u64) -> Result<(), String> {
        let entry = self
            .inner
            .servers
            .lock()
            .map_err(|_| "插件服务表不可用".to_string())?
            .remove(&id);
        let Some(entry) = entry else {
            return Ok(());
        };
        if entry.plugin != plugin {
            return Err("不能关闭其它插件的服务".into());
        }
        if let Some(shutdown) = entry.shutdown {
            let _ = shutdown.send(());
        }
        self.close_sockets(id);
        Ok(())
    }

    pub fn close_all(&self, plugin: &str) {
        let ids: Vec<u64> = self
            .inner
            .servers
            .lock()
            .map(|servers| {
                servers
                    .iter()
                    .filter(|(_, entry)| entry.plugin == plugin)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        for id in ids {
            let _ = self.close(plugin, id);
        }
        let clients: Vec<u64> = self
            .inner
            .clients
            .lock()
            .map(|clients| {
                clients
                    .iter()
                    .filter(|(_, entry)| entry.plugin == plugin)
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default();
        for id in clients {
            let _ = self.client_close(plugin, id);
        }
    }

    fn close_sockets(&self, server: u64) {
        let mut targets = Vec::new();
        if let Ok(mut sockets) = self.inner.sockets.lock() {
            let ids: Vec<(u64, u64)> = sockets
                .iter()
                .filter(|((sid, _), _)| *sid == server)
                .map(|(key, _)| *key)
                .collect();
            for key in ids {
                if let Some(handle) = sockets.remove(&key) {
                    targets.push(handle.tx);
                }
            }
        }
        for tx in targets {
            let _ = tx.send(SocketCommand::Close);
        }
    }

    pub fn respond(
        &self,
        plugin: &str,
        request: u64,
        status: u16,
        headers: &str,
        body: &str,
        end: bool,
    ) -> Result<bool, String> {
        let handle = self
            .inner
            .responses
            .lock()
            .map_err(|_| "插件响应表不可用".to_string())?
            .get(&request)
            .map(|handle| (handle.plugin.clone(), handle.tx.clone()));
        let Some((owner, tx)) = handle else {
            return Ok(false);
        };
        if owner != plugin {
            return Err("不能响应其它插件的请求".into());
        }
        let headers = serde_json::from_str::<Value>(headers)
            .ok()
            .and_then(|value| {
                value.as_object().map(|object| {
                    object
                        .iter()
                        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        let action = ResponseAction::Chunk {
            status,
            headers,
            body: body.as_bytes().to_vec(),
            end,
        };
        tx.send(action).map_err(|_| "请求已经结束".to_string())?;
        if end {
            if let Ok(mut responses) = self.inner.responses.lock() {
                responses.remove(&request);
            }
        }
        Ok(true)
    }

    pub fn ws_send(
        &self,
        plugin: &str,
        server: u64,
        connection: u64,
        text: &str,
    ) -> Result<bool, String> {
        let tx = self
            .inner
            .sockets
            .lock()
            .map_err(|_| "插件连接表不可用".to_string())?
            .get(&(server, connection))
            .map(|handle| (handle.plugin.clone(), handle.tx.clone()));
        let Some((owner, tx)) = tx else {
            return Ok(false);
        };
        if owner != plugin {
            return Err("不能操作其它插件的连接".into());
        }
        tx.send(SocketCommand::Text(text.to_string()))
            .map_err(|_| "连接已经关闭".to_string())?;
        Ok(true)
    }

    pub fn ws_close(&self, plugin: &str, server: u64, connection: u64) -> Result<bool, String> {
        let tx = self
            .inner
            .sockets
            .lock()
            .map_err(|_| "插件连接表不可用".to_string())?
            .remove(&(server, connection))
            .map(|handle| (handle.plugin.clone(), handle.tx));
        let Some((owner, tx)) = tx else {
            return Ok(false);
        };
        if owner != plugin {
            return Err("不能操作其它插件的连接".into());
        }
        let _ = tx.send(SocketCommand::Close);
        Ok(true)
    }

    /// 取走这个插件的全部待处理事件。别的插件的事件原样留在队列里。
    pub fn poll(&self, plugin: &str) -> String {
        let mut out = Vec::new();
        if let Ok(mut events) = self.inner.events.lock() {
            let mut keep = Vec::with_capacity(events.len());
            for (owner, event) in events.drain(..) {
                if owner == plugin {
                    out.push(event);
                } else {
                    keep.push((owner, event));
                }
            }
            *events = keep;
        }
        Value::Array(out).to_string()
    }

    /// WebSocket 客户端：连上之后返回 id，收发事件走 poll()。
    pub fn client_connect(&self, plugin: &str, url: &str) -> Result<Value, String> {
        let target = url.to_string();
        let (socket, _) = crate::server::bridge_block_on(|| async move {
            tokio_tungstenite::connect_async(target.as_str())
                .await
                .map_err(|error| format!("连接 WebSocket 失败：{error}"))
        })?;
        let id = self.next_client();
        let (tx, mut rx) = mpsc::unbounded_channel::<SocketCommand>();
        if let Ok(mut clients) = self.inner.clients.lock() {
            clients.insert(
                id,
                ClientHandle {
                    plugin: plugin.to_string(),
                    tx,
                },
            );
        }
        self.push_event(
            plugin,
            json!({ "type": "ws-open", "source": "client", "client": id }),
        );
        let (mut sink, mut stream) = socket.split();
        let manager = self.clone();
        let owner = plugin.to_string();
        crate::server::bridge_runtime().spawn(async move {
            while let Some(command) = rx.recv().await {
                match command {
                    SocketCommand::Text(text) => {
                        if sink.send(ClientMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    SocketCommand::Close => {
                        let _ = sink.send(ClientMessage::Close(None)).await;
                        break;
                    }
                }
            }
        });
        let manager_reader = manager.clone();
        let owner_reader = owner.clone();
        crate::server::bridge_runtime().spawn(async move {
            while let Some(message) = stream.next().await {
                match message {
                    Ok(ClientMessage::Text(text)) => {
                        manager_reader.push_event(
                            &owner_reader,
                            json!({ "type": "ws-message", "source": "client", "client": id, "data": text.to_string() }),
                        );
                    }
                    Ok(ClientMessage::Binary(bytes)) => {
                        manager_reader.push_event(
                            &owner_reader,
                            json!({ "type": "ws-message", "source": "client", "client": id, "data": String::from_utf8_lossy(&bytes) }),
                        );
                    }
                    Ok(ClientMessage::Close(frame)) => {
                        let code = frame.as_ref().map(|frame| u16::from(frame.code)).unwrap_or(1005);
                        manager_reader.push_event(
                            &owner_reader,
                            json!({ "type": "ws-close", "source": "client", "client": id, "code": code }),
                        );
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if let Ok(mut clients) = manager_reader.inner.clients.lock() {
                clients.remove(&id);
            }
        });
        Ok(json!({ "id": id }))
    }

    pub fn client_send(&self, plugin: &str, id: u64, text: &str) -> Result<bool, String> {
        let tx = self
            .inner
            .clients
            .lock()
            .map_err(|_| "WebSocket 客户端表不可用".to_string())?
            .get(&id)
            .map(|handle| (handle.plugin.clone(), handle.tx.clone()));
        let Some((owner, tx)) = tx else {
            return Ok(false);
        };
        if owner != plugin {
            return Err("不能操作其它插件的 WebSocket 客户端".into());
        }
        tx.send(SocketCommand::Text(text.to_string()))
            .map_err(|_| "WebSocket 已经关闭".to_string())?;
        Ok(true)
    }

    pub fn client_close(&self, plugin: &str, id: u64) -> Result<bool, String> {
        let tx = self
            .inner
            .clients
            .lock()
            .map_err(|_| "WebSocket 客户端表不可用".to_string())?
            .remove(&id)
            .map(|handle| (handle.plugin.clone(), handle.tx));
        let Some((owner, tx)) = tx else {
            return Ok(false);
        };
        if owner != plugin {
            return Err("不能操作其它插件的 WebSocket 客户端".into());
        }
        let _ = tx.send(SocketCommand::Close);
        Ok(true)
    }

    async fn handle_socket(&self, server: u64, socket: WebSocket, path: String) {
        let connection = self.next_connection();
        let (mut sink, mut stream) = socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<SocketCommand>();
        let plugin = self
            .inner
            .servers
            .lock()
            .ok()
            .and_then(|servers| servers.get(&server).map(|entry| entry.plugin.clone()))
            .unwrap_or_default();
        if plugin.is_empty() {
            return;
        }
        if let Ok(mut sockets) = self.inner.sockets.lock() {
            sockets.insert(
                (server, connection),
                SocketHandle {
                    plugin: plugin.clone(),
                    tx,
                },
            );
        }
        self.push_event(
            &plugin,
            json!({ "type": "ws-open", "source": "server", "server": server, "conn": connection, "path": path }),
        );
        crate::server::bridge_runtime().spawn(async move {
            while let Some(command) = rx.recv().await {
                match command {
                    SocketCommand::Text(text) => {
                        if sink.send(AxumMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    SocketCommand::Close => {
                        let _ = sink.send(AxumMessage::Close(None)).await;
                        break;
                    }
                }
            }
        });
        while let Some(message) = stream.next().await {
            match message {
                Ok(AxumMessage::Text(text)) => {
                    self.push_event(
                        &plugin,
                        json!({ "type": "ws-message", "source": "server", "server": server, "conn": connection, "data": text.to_string() }),
                    );
                }
                Ok(AxumMessage::Binary(bytes)) => {
                    self.push_event(
                        &plugin,
                        json!({ "type": "ws-message", "source": "server", "server": server, "conn": connection, "data": String::from_utf8_lossy(&bytes) }),
                    );
                }
                Ok(AxumMessage::Close(frame)) => {
                    let code = frame
                        .as_ref()
                        .map(|frame| u16::from(frame.code))
                        .unwrap_or(1005);
                    self.push_event(
                        &plugin,
                        json!({ "type": "ws-close", "source": "server", "server": server, "conn": connection, "code": code }),
                    );
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if let Ok(mut sockets) = self.inner.sockets.lock() {
            sockets.remove(&(server, connection));
        }
    }
}

async fn handle_entry(State(ctx): State<AppCtx>, request: Request) -> Response {
    let (mut parts, body) = request.into_parts();
    if is_websocket_upgrade(&parts.headers) {
        let path = parts.uri.path().to_string();
        return match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(upgrade) => upgrade.on_upgrade(move |socket| async move {
                ctx.servers.handle_socket(ctx.server, socket, path).await;
            }),
            Err(error) => error.into_response(),
        };
    }

    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = headers_json(&parts.headers);
    let body = match to_bytes(body, MAX_BODY).await {
        Ok(value) => value,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
    };
    let plugin = ctx
        .servers
        .inner
        .servers
        .lock()
        .ok()
        .and_then(|servers| servers.get(&ctx.server).map(|entry| entry.plugin.clone()))
        .unwrap_or_default();
    if plugin.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is closed").into_response();
    }
    let request_id = ctx.servers.next_request();
    let (tx, mut rx) = mpsc::unbounded_channel::<ResponseAction>();
    if let Ok(mut responses) = ctx.servers.inner.responses.lock() {
        responses.insert(
            request_id,
            RequestHandle {
                plugin: plugin.clone(),
                tx,
            },
        );
    }
    ctx.servers.push_event(
        &plugin,
        json!({
            "type": "http-request",
            "server": ctx.server,
            "id": request_id,
            "method": method,
            "path": path,
            "query": query,
            "headers": headers,
            "body": String::from_utf8_lossy(&body),
        }),
    );

    let first = match tokio::time::timeout(RESPONSE_TIMEOUT, rx.recv()).await {
        Ok(Some(action)) => action,
        Ok(None) => {
            return (StatusCode::BAD_GATEWAY, "plugin closed the response").into_response();
        }
        Err(_) => {
            if let Ok(mut responses) = ctx.servers.inner.responses.lock() {
                responses.remove(&request_id);
            }
            return (StatusCode::GATEWAY_TIMEOUT, "plugin did not respond").into_response();
        }
    };
    let ResponseAction::Chunk {
        status,
        headers,
        body,
        end,
    } = first;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if end {
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = status;
        apply_headers(response.headers_mut(), headers);
        return response;
    }

    let (body_tx, body_rx) = mpsc::channel::<Result<bytes::Bytes, Infallible>>(32);
    let manager = ctx.servers.clone();
    crate::server::bridge_runtime().spawn(async move {
        if !body.is_empty() {
            let _ = body_tx.send(Ok(bytes::Bytes::from(body))).await;
        }
        while let Some(action) = rx.recv().await {
            let ResponseAction::Chunk {
                headers: _,
                body,
                end,
                ..
            } = action;
            if !body.is_empty() && body_tx.send(Ok(bytes::Bytes::from(body))).await.is_err() {
                break;
            }
            if end {
                break;
            }
        }
        if let Ok(mut responses) = manager.inner.responses.lock() {
            responses.remove(&request_id);
        }
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(body_rx)));
    *response.status_mut() = status;
    apply_headers(response.headers_mut(), headers);
    response
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

fn headers_json(headers: &HeaderMap) -> Value {
    let mut out = serde_json::Map::new();
    for (key, value) in headers {
        out.insert(
            key.as_str().to_string(),
            Value::String(value.to_str().unwrap_or("").to_string()),
        );
    }
    Value::Object(out)
}

fn apply_headers(target: &mut HeaderMap, headers: Vec<(String, String)>) {
    for (key, value) in headers {
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };
        target.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn http_server_roundtrip_reaches_plugin() {
        let servers = PluginServers::new();
        let info = servers.listen("test", "127.0.0.1", 0).expect("listen");
        let port = info["port"].as_u64().expect("port") as u16;
        let (tx, rx) = oneshot::channel::<(u16, String)>();
        crate::server::bridge_runtime().spawn(async move {
            let response = reqwest::Client::new()
                .get(format!("http://127.0.0.1:{port}/hello?x=1"))
                .send()
                .await;
            let result = match response {
                Ok(response) => (
                    response.status().as_u16(),
                    response.text().await.unwrap_or_default(),
                ),
                Err(error) => (0, error.to_string()),
            };
            let _ = tx.send(result);
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut request = None;
        while Instant::now() < deadline {
            let events: Value = serde_json::from_str(&servers.poll("test")).expect("events");
            if let Some(event) = events.as_array().and_then(|items| items.first()) {
                request = Some(event.clone());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let request = request.expect("request event");
        assert_eq!(request["path"], "/hello");
        assert_eq!(request["query"], "x=1");
        let id = request["id"].as_u64().expect("request id");
        servers
            .respond(
                "test",
                id,
                201,
                r#"{"x-test":"yes"}"#,
                "hello from plugin",
                true,
            )
            .expect("respond");

        let (status, body) = crate::server::bridge_block_on(|| rx).expect("response");
        assert_eq!(status, 201);
        assert_eq!(body, "hello from plugin");
        servers.close_all("test");
    }

    #[test]
    fn a_plugin_cannot_touch_another_plugins_server() {
        let servers = PluginServers::new();
        let info = servers.listen("owner", "127.0.0.1", 0).expect("listen");
        let id = info["id"].as_u64().expect("id");
        assert!(servers.close("other", id).is_err());
    }
}
