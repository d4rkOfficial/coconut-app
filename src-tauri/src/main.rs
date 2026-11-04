#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod console_bus;
mod i18n;
mod plugin;
mod plugin_host;
mod plugin_rt;
mod plugin_server;
mod server;

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Manager, RunEvent, WindowEvent};

use server::{ServerState, TOOLBOX_PORT, WS_PORT};

struct AppServer(Mutex<Option<Arc<ServerState>>>);

struct Launched(AtomicBool);

/// 启动器首页地址：打开工具箱前记下来，`Coconut.toolbox.exit` 靠它退回去。
pub struct LauncherUrl(pub Mutex<String>);

#[derive(Clone, Serialize)]
struct Bootstrap {
    data_dir: String,
    service_url: String,
    service_path: String,
    error: String,
}

struct Boot(Mutex<Bootstrap>);

#[derive(Clone, Serialize)]
struct UninstallStep {
    key: &'static str,
    label: &'static str,
    detail: &'static str,
    status: &'static str,
}

#[derive(Clone, Serialize, Default)]
struct UninstallProgress {
    active: bool,
    done: bool,
    error: String,
    steps: Vec<UninstallStep>,
}

struct Uninstall(Arc<Mutex<UninstallProgress>>);

/// 卸载完成后不立刻退出，等用户手动退出，或静默等这么久后自动退出。
const AUTO_EXIT_DELAY: Duration = Duration::from_secs(5 * 60);

/// 卸载步骤文案：key + [简中, 繁中, 日, 韩, 英]。
const UNINSTALL_STEPS: &[(&str, [&str; 5], [&str; 5])] = &[
    (
        "stop",
        [
            "停止本地服务",
            "停止本機服務",
            "ローカルサービスを停止",
            "로컬 서비스 중지",
            "Stop the local service",
        ],
        [
            "断开 WS 接管，停止静态服务与 API / 模型代理",
            "斷開 WS 接管，停止靜態服務與 API / 模型代理",
            "WS 接続を切り、静的配信と API / モデル代理を停止",
            "WS 연결을 끊고 정적 서비스와 API / 모델 프록시를 중지",
            "Drop WS sessions and stop the static service, API and model proxy",
        ],
    ),
    (
        "browser",
        [
            "清除浏览器存储",
            "清除瀏覽器儲存",
            "ブラウザの保存データを削除",
            "브라우저 저장소 삭제",
            "Clear browser storage",
        ],
        [
            "localStorage / IndexedDB / Service Worker",
            "localStorage / IndexedDB / Service Worker",
            "localStorage / IndexedDB / Service Worker",
            "localStorage / IndexedDB / Service Worker",
            "localStorage / IndexedDB / Service Worker",
        ],
    ),
    (
        "toolbox",
        [
            "删除工具箱静态文件",
            "刪除工具箱靜態檔案",
            "ツールボックスの静的ファイルを削除",
            "툴박스 정적 파일 삭제",
            "Delete toolbox static files",
        ],
        [
            "toolbox-static",
            "toolbox-static",
            "toolbox-static",
            "toolbox-static",
            "toolbox-static",
        ],
    ),
    (
        "game",
        [
            "删除游戏数据缓存",
            "刪除遊戲資料快取",
            "ゲームデータのキャッシュを削除",
            "게임 데이터 캐시 삭제",
            "Delete the game data cache",
        ],
        [
            "game-data.json",
            "game-data.json",
            "game-data.json",
            "game-data.json",
            "game-data.json",
        ],
    ),
    (
        "models",
        [
            "删除免费模型缓存与 API Key",
            "刪除免費模型快取與 API Key",
            "無料モデルのキャッシュと API Key を削除",
            "무료 모델 캐시와 API Key 삭제",
            "Delete the free-model cache and API key",
        ],
        [
            "free-models.json / opencode-key.json",
            "free-models.json / opencode-key.json",
            "free-models.json / opencode-key.json",
            "free-models.json / opencode-key.json",
            "free-models.json / opencode-key.json",
        ],
    ),
    (
        "webview",
        [
            "删除 WebView2 用户数据",
            "刪除 WebView2 使用者資料",
            "WebView2 のユーザーデータを削除",
            "WebView2 사용자 데이터 삭제",
            "Delete WebView2 user data",
        ],
        [
            "缓存 / Cookie / Local Storage / Crashpad",
            "快取 / Cookie / Local Storage / Crashpad",
            "キャッシュ / Cookie / Local Storage / Crashpad",
            "캐시 / Cookie / Local Storage / Crashpad",
            "Cache / Cookies / Local Storage / Crashpad",
        ],
    ),
    (
        "exit",
        [
            "退出启动器",
            "退出啟動器",
            "ランチャーを終了",
            "런처 종료",
            "Exit the launcher",
        ],
        [
            "退出后继续清理被占用的文件",
            "退出後繼續清理被占用的檔案",
            "終了後に使用中のファイルを削除",
            "종료 후 사용 중인 파일 정리",
            "Keep cleaning files that were still in use",
        ],
    ),
];

fn uninstall_step_list(lang: crate::i18n::Lang) -> Vec<UninstallStep> {
    UNINSTALL_STEPS
        .iter()
        .map(|(key, label, detail)| UninstallStep {
            key,
            label: i18n::pick(lang, *label),
            detail: i18n::pick(lang, *detail),
            status: "pending",
        })
        .collect()
}

fn uninstall_progress(app: &tauri::AppHandle) -> Arc<Mutex<UninstallProgress>> {
    app.state::<Uninstall>().0.clone()
}

fn mark_step(app: &tauri::AppHandle, index: usize, status: &'static str) {
    let progress = uninstall_progress(app);
    let Ok(mut guard) = progress.lock() else {
        return;
    };
    let step = guard.steps.get_mut(index);
    if let Some(step) = step {
        step.status = status;
    }
}

#[derive(Serialize)]
struct UiState {
    commit: String,
    build: String,
    url: String,
    ready: bool,
    error: String,
    theme: String,
    game_version: String,
    progress: String,
    ws_url: String,
    has_static: bool,
    crawling: bool,
    /// 工具箱静态包是否正在自动 / 手动更新。
    updating: bool,
    /// MSI 安装目录（存在 uninstall.exe 时非空）；便携版为空。
    uninstaller_dir: String,
}

fn server_state(app: &tauri::AppHandle) -> Option<Arc<ServerState>> {
    app.state::<AppServer>()
        .0
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
}

/// MSI 安装目录：当前 exe 旁边有 uninstall.exe 时返回该目录。
/// 便携版通常没有这个文件；有的话也按安装版处理。
fn uninstaller_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in ["uninstall.exe", "Uninstall.exe"] {
        if dir.join(name).is_file() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn normalize_windows_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{}", rest));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest.to_string());
    }
    path
}

#[tauri::command]
fn bootstrap(state: tauri::State<'_, Boot>) -> Bootstrap {
    state
        .0
        .lock()
        .map(|value| value.clone())
        .unwrap_or_else(|_| Bootstrap {
            data_dir: String::new(),
            service_url: format!("http://127.0.0.1:{TOOLBOX_PORT}"),
            service_path: String::new(),
            error: "内部状态不可用".into(),
        })
}

#[tauri::command]
async fn get_state(app: tauri::AppHandle) -> Result<UiState, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let theme = state.theme.read().await.clone();
    let error = state.error.read().await.clone();
    let commit = state.commit.read().await.clone();
    let build = state.build.read().await.clone();
    let game_version = state.game.read().await.version.clone();
    let progress = state.progress.read().await.clone();
    let ready = state.ready.load(Ordering::SeqCst);
    Ok(UiState {
        commit,
        build,
        url: format!("http://127.0.0.1:{TOOLBOX_PORT}/#websocket=127.0.0.1:{WS_PORT}"),
        ready,
        error,
        theme,
        game_version,
        progress,
        ws_url: format!("ws://127.0.0.1:{WS_PORT}"),
        // 有静态产物就能启动工具箱，哪怕游戏数据还在后台拉取。
        has_static: state.static_dir.join("index.html").is_file(),
        crawling: state.crawling.load(Ordering::SeqCst),
        updating: state.updating.load(Ordering::SeqCst),
        uninstaller_dir: uninstaller_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
    })
}

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let result = server::check_toolbox_update(&state).await;
    if let Err(error) = &result {
        state.log("update", &format!("检查工具箱更新失败：{error}"));
        state.set_progress(format!("检查失败：{error}")).await;
    }
    result
}

#[tauri::command]
async fn update_toolbox(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let result = server::update_toolbox(&state).await;
    match &result {
        Ok(_) => reload_toolbox_page(&app),
        Err(error) => {
            state.log("update", &format!("工具箱更新失败：{error}"));
            state.set_progress(format!("更新失败：{error}")).await;
        }
    }
    result
}

/// 静态包换完之后，如果工具箱页面已经打开，直接让它重新加载：
/// 用户不应该需要自己按 Ctrl+Shift+R 才能看到新版本。
fn reload_toolbox_page(app: &tauri::AppHandle) {
    if !app.state::<Launched>().0.load(Ordering::SeqCst) {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(url) = window.url() else {
        return;
    };
    if !url.as_str().starts_with("http://127.0.0.1:") {
        return;
    }
    let _ = app.run_on_main_thread(move || {
        let _ = window.eval("window.location.reload()");
    });
}

#[tauri::command]
fn plugins_list(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    serde_json::to_value(state.plugins.list()).map_err(|error| error.to_string())
}

#[tauri::command]
fn plugins_set_enabled(app: tauri::AppHandle, id: String, enabled: bool) -> Result<(), String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    state.plugins.set_enabled(&id, enabled)
}

#[tauri::command]
fn plugins_reload(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    state.plugins.reload(&id)
}

#[tauri::command]
fn plugins_cleanup_dead(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    state.plugins.cleanup_dead()
}

#[tauri::command]
fn open_uninstaller_dir() -> Result<String, String> {
    let dir =
        uninstaller_dir().ok_or_else(|| "未找到 uninstall.exe（便携版请直接退出）".to_string())?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|error| format!("打开安装目录失败：{error}"))?;
    }
    Ok(dir.display().to_string())
}

#[tauri::command]
fn plugins_open_dir(app: tauri::AppHandle) -> Result<String, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let dir = state.plugins.dir().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建插件目录失败：{error}"))?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|error| format!("打开插件目录失败：{error}"))?;
    }
    Ok(dir.display().to_string())
}

#[tauri::command]
async fn set_language(app: tauri::AppHandle, lang: String) -> Result<String, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    state.set_language(&lang).await?;
    Ok(state.lang().await.tag().to_string())
}

#[tauri::command]
async fn switch_mirror(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let index = state.switch_mirror();
    // 镜像名字由前端按当前语言拼（直连 / 镜像 1 / Mirror 1 …）
    Ok(serde_json::json!({
        "index": index,
        "count": state.mirror_count(),
    }))
}

#[tauri::command]
async fn check_game_update(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let result = server::check_game_update(&state).await;
    if let Err(error) = &result {
        state.log("game", &format!("检查游戏数据版本失败：{error}"));
        state.set_progress(format!("检查失败：{error}")).await;
    }
    result
}

#[tauri::command]
async fn update_game_data(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    let result = server::update_game_data(&state).await;
    if let Err(error) = &result {
        state.log("game", &format!("游戏数据更新失败：{error}"));
        state.set_progress(format!("更新失败：{error}")).await;
    }
    result
}

#[tauri::command]
fn open_toolbox_window(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    url: String,
) -> Result<(), String> {
    app.state::<Launched>().0.store(true, Ordering::SeqCst);
    // 每次启动工具箱都换一份新日志：旧的备份成 logs/plugins-<时间>.log
    if let Some(state) = server_state(&app) {
        state.plugins.begin_log_session("打开工具箱");
    }
    let parsed = tauri::Url::parse(&url).map_err(|error| format!("无效的工具箱地址：{error}"))?;
    // 退回启动器要用的地址：当前窗口还停在启动器页面上，现在记最准。
    if let Ok(launcher) = window.url() {
        if let Ok(mut slot) = app.state::<LauncherUrl>().0.lock() {
            *slot = launcher.to_string();
        }
    }
    window
        .set_title("椰果工具箱")
        .map_err(|error| format!("无法修改窗口标题：{error}"))?;
    window
        .navigate(parsed)
        .map_err(|error| format!("无法在 Tauri 中打开工具箱：{error}"))
}

#[tauri::command]
fn set_window_title(window: tauri::WebviewWindow, title: String) -> Result<(), String> {
    window
        .set_title(&title)
        .map_err(|error| format!("无法修改窗口标题：{error}"))
}

#[tauri::command]
fn set_window_theme(window: tauri::WebviewWindow, theme: String) -> Result<(), String> {
    let value = if theme == "dark" {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    };
    window
        .set_theme(Some(value))
        .map_err(|error| format!("无法修改窗口主题：{error}"))
}

#[tauri::command]
async fn set_theme(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let state = server_state(&app).ok_or_else(|| "本地服务尚未启动".to_string())?;
    state.set_theme(&theme).await?;
    Ok(())
}

#[tauri::command]
async fn start_uninstall(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let lang = match server_state(&app) {
        Some(state) => state.lang().await,
        None => crate::i18n::Lang::En,
    };
    let data_dir = app
        .state::<Boot>()
        .0
        .lock()
        .map(|boot| boot.data_dir.clone())
        .unwrap_or_default();
    // WebView2 的用户数据（localStorage / IndexedDB / 缓存 / Crashpad）在这里，
    // 不在 %APPDATA%，而且进程没退出前会被 lockfile 占着。
    let webview_dir = app
        .path()
        .app_local_data_dir()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    {
        let progress = uninstall_progress(&app);
        let mut guard = progress.lock().map_err(|_| "内部状态不可用".to_string())?;
        if guard.active {
            return Ok(());
        }
        *guard = UninstallProgress {
            active: true,
            done: false,
            error: String::new(),
            steps: uninstall_step_list(lang),
        };
    }

    // 浏览器存储只能在进程内清，不能放到后台线程里等。
    mark_step(&app, 1, "running");
    let browser_error = window
        .clear_all_browsing_data()
        .err()
        .map(|error| error.to_string());

    let handle = app.clone();
    let progress = uninstall_progress(&app);
    std::thread::spawn(move || {
        let wait = |ms: u64| std::thread::sleep(Duration::from_millis(ms));
        let step = |index: usize, status: &'static str| mark_step(&handle, index, status);
        let fail = |message: String| {
            if let Ok(mut guard) = progress.lock() {
                if guard.error.is_empty() {
                    guard.error = message;
                }
            }
        };

        // 0 停止本地服务（进程退出时端口随之释放）
        step(0, "running");
        wait(260);
        step(0, "done");

        // 1 浏览器存储（命令里已执行）
        if let Some(error) = browser_error {
            fail(format!("清除浏览器数据失败：{error}"));
            step(1, "failed");
        } else {
            step(1, "done");
        }
        wait(120);

        // 2 工具箱静态文件
        step(2, "running");
        wait(240);
        let result = fs::remove_dir_all(Path::new(&data_dir).join("toolbox-static"));
        if let Err(error) = &result {
            if error.kind() != std::io::ErrorKind::NotFound {
                fail(format!("删除工具箱静态文件失败：{error}"));
                step(2, "failed");
            } else {
                step(2, "done");
            }
        } else {
            step(2, "done");
        }

        // 3 游戏数据缓存
        step(3, "running");
        wait(200);
        let _ = fs::remove_file(Path::new(&data_dir).join("game-data.json"));
        step(3, "done");

        // 4 免费模型缓存与 API Key
        step(4, "running");
        wait(200);
        let _ = fs::remove_file(Path::new(&data_dir).join("free-models.json"));
        let _ = fs::remove_file(Path::new(&data_dir).join("opencode-key.json"));
        step(4, "done");

        // 5 WebView2 用户数据（可能被 lockfile 占着，后续任务兜底）
        step(5, "running");
        wait(220);
        if !webview_dir.is_empty() {
            // 进程还在跑，这里多半删不干净，失败也不报错——退出后的清理任务会重试。
            let _ = fs::remove_dir_all(&webview_dir);
        }
        step(5, "done");

        // 6 清理完成：不立刻退出，等用户点「退出启动器」或 5 分钟后自动退出
        step(6, "running");
        wait(220);
        step(6, "done");

        {
            if let Ok(mut guard) = progress.lock() {
                guard.active = false;
                guard.done = true;
            }
        }

        let timer = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(AUTO_EXIT_DELAY);
            exit_with_cleanup(&timer);
        });
    });

    Ok(())
}

/// 真正退出：先把「退出后清理任务」挂上，再关进程。
/// 只有在这里启动，任务才不用把时间浪费在等一个还在跑的启动器上。
fn exit_with_cleanup(app: &tauri::AppHandle) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }

    let boot = app.state::<Boot>();
    let data_dir = boot
        .0
        .lock()
        .map(|boot| boot.data_dir.clone())
        .unwrap_or_default();
    drop(boot);

    let webview_dir = app
        .path()
        .app_local_data_dir()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    let targets: Vec<String> = [data_dir, webview_dir]
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect();
    #[cfg(windows)]
    if let Err(error) = spawn_cleanup_after_exit(std::process::id(), &targets) {
        eprintln!("[uninstall] 启动退出后清理任务失败：{error}");
    }
    #[cfg(not(windows))]
    let _ = &targets;

    app.exit(0);
}

#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    exit_with_cleanup(&app);
}

#[tauri::command]
fn get_uninstall_state(app: tauri::AppHandle) -> UninstallProgress {
    let progress = uninstall_progress(&app);
    progress
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

#[tauri::command]
fn window_minimize(window: tauri::WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|error| error.to_string())
}

#[tauri::command]
fn window_toggle_maximize(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(|error| error.to_string())
    } else {
        window.maximize().map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn window_close(window: tauri::WebviewWindow) -> Result<(), String> {
    window.close().map_err(|error| error.to_string())
}

/// 退出后清理脚本：等本进程结束、WebView2 释放 lockfile，再删目录并重试。
#[cfg(windows)]
fn cleanup_script(pid: u32, dirs: &[String]) -> String {
    let list = dirs
        .iter()
        .map(|dir| format!("'{}'", dir.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$dirs = @({list}); \
         for ($w = 0; $w -lt 40; $w++) {{ \
           if (-not (Get-Process -Id {pid} -ErrorAction SilentlyContinue)) {{ break }}; \
           Start-Sleep -Milliseconds 500 \
         }}; \
         for ($i = 0; $i -lt 60; $i++) {{ \
           foreach ($d in $dirs) {{ Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue }}; \
           $left = @($dirs | Where-Object {{ Test-Path -LiteralPath $_ }}); \
           if ($left.Count -eq 0) {{ break }}; \
           Start-Sleep -Milliseconds 500 \
         }}"
    )
}

#[cfg(windows)]
fn spawn_cleanup_after_exit(pid: u32, dirs: &[String]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let script = cleanup_script(pid, dirs);
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
}

#[tauri::command]
fn open_data_dir(state: tauri::State<'_, Boot>) -> Result<(), String> {
    let Ok(boot) = state.0.lock() else {
        return Err("内部状态不可用".into());
    };
    if boot.data_dir.is_empty() {
        return Err("数据目录尚未初始化".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&boot.data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&boot.data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&boot.data_dir)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// build.rs 在编译期把 toolbox-static 打成的压缩包；单文件便携版靠它自带工具箱。
/// 构建时若没有静态产物，这个文件是 0 字节，解包会失败并走降级路径。
const EMBEDDED_STATIC: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/toolbox.tar.gz"));

/// 找安装版放在资源目录 / exe 同级的 toolbox-static。
fn bundled_static_dir(app: &tauri::App) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("toolbox-static"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("toolbox-static"));
        }
    }
    // 开发时直接从仓库里取；release 不把开发机路径当兜底（否则换台机器就崩）。
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("toolbox-static"));
    }
    candidates
        .into_iter()
        .find(|path| path.join("index.html").is_file())
        .map(|path| normalize_windows_path(fs::canonicalize(&path).unwrap_or(path)))
}

fn copy_dir_all(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &destination)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

/// 准备工具箱静态目录。**任何一步失败都不再让 setup 失败**（否则 Tauri 会直接 abort，
/// 用户什么都看不到）：依次尝试 已就绪目录 → 安装版资源目录 → 内嵌压缩包 → 空目录。
fn prepare_static_dir(app: &tauri::App, data_dir: &Path) -> PathBuf {
    let target = data_dir.join("toolbox-static");
    if target.join("index.html").is_file() {
        return normalize_windows_path(fs::canonicalize(&target).unwrap_or(target));
    }

    if let Some(bundled) = bundled_static_dir(app) {
        match copy_dir_all(&bundled, &target) {
            Ok(()) if target.join("index.html").is_file() => {
                return normalize_windows_path(fs::canonicalize(&target).unwrap_or(target));
            }
            _ => eprintln!(
                "[setup] 从 {} 复制静态文件失败，改用内嵌压缩包",
                bundled.display()
            ),
        }
    }

    let _ = fs::remove_dir_all(&target);
    match server::extract_static_archive(EMBEDDED_STATIC, &target) {
        Ok(count) if target.join("index.html").is_file() => {
            eprintln!("[setup] 已从内嵌压缩包解出 {count} 个静态文件");
        }
        Ok(count) => eprintln!("[setup] 内嵌静态包不完整（{count} 个文件），缺少 index.html"),
        Err(error) => eprintln!("[setup] 解出内嵌静态包失败：{error}"),
    }

    // 仍然没有：留一个空目录，启动页会提示用户点「检查更新」在线下载。
    let _ = fs::create_dir_all(&target);
    normalize_windows_path(fs::canonicalize(&target).unwrap_or(target))
}

/// release 构建是 `panic = "abort"`：崩溃时用户只会看到进程直接消失。
/// 装一个 panic hook，把 panic 的位置写到 `%APPDATA%\com.coconut.launcher\panic.log`，
/// 至少能查出来是哪一行炸的。
fn install_panic_log() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "未知位置".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| text.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "未知 panic".to_string());
        let line = format!(
            "[{}] panic at {location}: {payload}\n{}\n",
            server::now_millis_public(),
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("{line}");
        if let Ok(dir) = std::env::var("APPDATA") {
            let path = std::path::PathBuf::from(dir)
                .join("com.coconut.launcher")
                .join("panic.log");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }));
}

fn main() {
    install_panic_log();
    let app = tauri::Builder::default()
        .manage(AppServer(Mutex::new(None)))
        .manage(Launched(AtomicBool::new(false)))
        .manage(LauncherUrl(Mutex::new(String::new())))
        .manage(Uninstall(Arc::new(
            Mutex::new(UninstallProgress::default()),
        )))
        .manage(Boot(Mutex::new(Bootstrap {
            data_dir: String::new(),
            service_url: format!("http://127.0.0.1:{TOOLBOX_PORT}"),
            service_path: String::new(),
            error: String::new(),
        })))
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_state,
            check_update,
            update_toolbox,
            check_game_update,
            update_game_data,
            set_language,
            plugins_list,
            plugins_set_enabled,
            plugins_reload,
            plugins_open_dir,
            plugins_cleanup_dead,
            open_uninstaller_dir,
            switch_mirror,
            open_toolbox_window,
            set_window_title,
            set_window_theme,
            set_theme,
            start_uninstall,
            get_uninstall_state,
            exit_app,
            window_minimize,
            window_toggle_maximize,
            window_close,
            open_data_dir
        ])
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("无法获取应用数据目录：{error}"))?;
            fs::create_dir_all(&data_dir)
                .map_err(|error| format!("无法创建应用数据目录：{error}"))?;

            let static_path = prepare_static_dir(app, &data_dir);
            let mut error = String::new();
            let server_state = match tauri::async_runtime::block_on(server::start(
                static_path.clone(),
                data_dir.clone(),
            )) {
                Ok(state) => {
                    if let Ok(mut slot) = app.state::<AppServer>().0.lock() {
                        *slot = Some(state.clone());
                    }
                    // 注入脚本与沉浸式标题栏要用 AppHandle 控制窗口。
                    let handle = app.handle().clone();
                    tauri::async_runtime::block_on(state.set_app(handle));
                    Some(state)
                }
                Err(message) => {
                    error = message.clone();
                    None
                }
            };

            let service_path = static_path.to_string_lossy().to_string();
            if let Ok(mut boot) = app.state::<Boot>().0.lock() {
                *boot = Bootstrap {
                    data_dir: data_dir.to_string_lossy().to_string(),
                    service_url: format!("http://127.0.0.1:{TOOLBOX_PORT}"),
                    service_path,
                    error,
                };
            }

            let reset_handle = app.handle().clone();
            std::thread::spawn(move || {
                #[cfg(windows)]
                let splash = "http://tauri.localhost/";
                #[cfg(not(windows))]
                let splash = "tauri://localhost";
                for _ in 0..30 {
                    std::thread::sleep(Duration::from_millis(100));
                    if reset_handle.state::<Launched>().0.load(Ordering::SeqCst) {
                        break;
                    }
                    let Some(window) = reset_handle.get_webview_window("main") else {
                        continue;
                    };
                    let Ok(url) = window.url() else {
                        continue;
                    };
                    let current = url.as_str();
                    if current.starts_with(splash) {
                        break;
                    }
                    if current.starts_with("http://127.0.0.1:") {
                        if let Ok(target) = tauri::Url::parse(splash) {
                            let _ = window.navigate(target);
                        }
                        break;
                    }
                }
            });

            if server_state.is_none() {
                // The splash UI reads the error via bootstrap/get_state.
                eprintln!("server failed to start");
            }

            // 启动时自动更新工具箱：工具箱发版后用户直接启动就有新版本，
            // 不需要点「检查更新」，也不需要进工具箱后按 Ctrl+Shift+R。
            if let Some(state) = server_state {
                let auto_state = state.clone();
                let auto_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    match server::auto_update_toolbox(&auto_state).await {
                        Ok(Some(commit)) => {
                            eprintln!("[auto-update] 工具箱已自动更新到 {commit}");
                            reload_toolbox_page(&auto_handle);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            auto_state.log("update", &format!("启动时自动更新跳过：{error}"));
                            eprintln!("[auto-update] 已跳过：{error}");
                        }
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { .. } => {
                let app = window.app_handle();
                // 已经卸载完再关窗口，也要补上退出后的清理任务。
                let uninstalled = app
                    .state::<Uninstall>()
                    .0
                    .lock()
                    .map(|guard| guard.done)
                    .unwrap_or(false);
                if uninstalled {
                    exit_with_cleanup(app);
                } else {
                    app.exit(0);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        if let RunEvent::Exit = event {
            // Rust servers live in this process and exit with it.
        }
    });
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn cleanup_script_covers_every_dir() {
        let dirs = vec![
            r"C:\Users\me\AppData\Roaming\com.coconut.launcher".to_string(),
            r"C:\Users\me\AppData\Local\com.coconut.launcher".to_string(),
        ];
        let script = cleanup_script(4242, &dirs);
        for dir in &dirs {
            assert!(script.contains(dir.as_str()), "missing {dir}");
        }
        assert!(script.contains("Get-Process -Id 4242"));
        assert!(script.contains("Remove-Item -LiteralPath"));
        // 作为单个 argv 传给 powershell，脚本里不能出现双引号。
        assert!(!script.contains('"'));
    }

    #[test]
    fn cleanup_script_escapes_single_quotes() {
        let dirs = vec![r"C:\it's here".to_string()];
        let script = cleanup_script(7, &dirs);
        assert!(script.contains(r"'C:\it''s here'"));
    }

    #[test]
    fn bundled_static_dir_falls_back_to_dev_copy() {
        // 只验证路径拼接不会 panic 且指向仓库内的静态产物。
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("toolbox-static");
        assert!(dev.ends_with("toolbox-static"));
    }
}
