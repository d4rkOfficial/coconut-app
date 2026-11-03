//! 插件系统：单个 `<名字>.js` 文件 + 头部元数据注释，在启动器进程内用 QuickJS 运行。
//!
//! 这个模块现在只负责「插件是什么」：扫描目录、解析头部元数据、读写启用状态。
//! 真正的 JS 运行时在 `plugin_rt.rs`。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 插件可以被授予的权限。`network` 不限制域名，`debug` 用来只读订阅原始 WS 帧。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Permission {
    /// Coconut.runJs：在工具箱页面里执行 JS
    Dom,
    /// Coconut.callTool：调用工具箱的 WS 工具
    Toolbox,
    /// Coconut.fetch：走启动器发 HTTP
    Network,
    /// Coconut.storage：插件私有 KV
    Storage,
    /// Coconut.ui：浮层 / 注入 CSS
    Ui,
    /// Coconut.http：在插件线程外开本地 HTTP / WebSocket 服务
    Server,
    /// 只读订阅原始 WS 帧（开发者用）
    Debug,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Dom => "dom",
            Permission::Toolbox => "toolbox",
            Permission::Network => "network",
            Permission::Storage => "storage",
            Permission::Ui => "ui",
            Permission::Server => "server",
            Permission::Debug => "debug",
        }
    }

    pub fn parse(value: &str) -> Option<Permission> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "dom" => Permission::Dom,
            "toolbox" | "tool" => Permission::Toolbox,
            "network" | "net" | "fetch" => Permission::Network,
            "storage" => Permission::Storage,
            "ui" => Permission::Ui,
            "server" | "http" => Permission::Server,
            "debug" => Permission::Debug,
            _ => return None,
        })
    }
}

/// 插件想怎么露脸：`none` 只写 DOM，`overlay` 用启动器提供的浮层容器。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mount {
    #[default]
    None,
    Overlay,
}

impl Mount {
    pub fn as_str(self) -> &'static str {
        match self {
            Mount::None => "none",
            Mount::Overlay => "overlay",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub mount: Mount,
}

impl PluginMeta {
    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }
}

/// 一个插件文件。内置插件没有路径（源码编进 exe）。
#[derive(Clone, Debug)]
pub struct Plugin {
    /// 文件名（不含 `.js`），同时作为插件 id
    pub id: String,
    pub path: Option<PathBuf>,
    pub source: String,
    pub meta: PluginMeta,
    pub builtin: bool,
}

/// 解析 `// @key value` 形式的头部元数据。
///
/// 只认文件开头到第一段代码之前的注释块，避免正文里的注释被误读。
pub fn parse_meta(source: &str, fallback_name: &str) -> PluginMeta {
    let mut meta = PluginMeta {
        name: fallback_name.to_string(),
        ..Default::default()
    };
    let mut permissions: Vec<Permission> = Vec::new();
    for raw in source.lines().take(40) {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("//") {
            break;
        }
        let body = line.trim_start_matches('/').trim();
        let Some(rest) = body.strip_prefix('@') else {
            continue;
        };
        let (key, value) = match rest.split_once(char::is_whitespace) {
            Some((key, value)) => (key, value.trim()),
            None => (rest.trim(), ""),
        };
        match key.to_ascii_lowercase().as_str() {
            "name" => meta.name = value.to_string(),
            "version" => meta.version = value.to_string(),
            "description" => meta.description = value.to_string(),
            "mount" => {
                meta.mount = match value.to_ascii_lowercase().as_str() {
                    "overlay" => Mount::Overlay,
                    _ => Mount::None,
                }
            }
            "permissions" | "permission" => {
                for item in value.split([',', ' ', '|']) {
                    if item.trim().is_empty() {
                        continue;
                    }
                    if let Some(permission) = Permission::parse(item) {
                        if !permissions.contains(&permission) {
                            permissions.push(permission);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    permissions.sort();
    meta.permissions = permissions;
    meta
}

/// 从磁盘加载一个插件文件。
pub fn load_plugin(path: &Path) -> Result<Plugin, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("读取插件 {} 失败：{error}", path.display()))?;
    // 记事本 / Windows PowerShell 写的 UTF-8 带 BOM：会顶掉第一行注释，
    // 让 @name 之类元数据整段失效，这里直接剥掉。
    let source = source.trim_start_matches('\u{feff}').to_string();
    let id = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if id.is_empty() {
        return Err("插件文件名不能为空".into());
    }
    let meta = parse_meta(&source, &id);
    Ok(Plugin {
        id,
        path: Some(path.to_path_buf()),
        source,
        meta,
        builtin: false,
    })
}

/// 扫描插件目录里的 `*.js`（按文件名排序，跳过以 `_` 开头的模板文件）。
pub fn scan_plugins(dir: &Path) -> Vec<Plugin> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "js").unwrap_or(false))
        .filter(|path| {
            !path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with('_'))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|path| load_plugin(path).ok())
        .collect()
}

/// 每个插件的启用状态 / 连续错误数，落在 `plugins/state.json`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 连续失败次数（成功一次就清零），到 3 次自动停用
    #[serde(default)]
    pub errors: u32,
    #[serde(default)]
    pub last_error: String,
    /// 手动「伪删除」标记：内置插件没有文件可删，把 state.json 里这个字段改成 true
    /// 就会从插件列表里消失（改回来就恢复）。启动器里没有对应的 UI。
    #[serde(default)]
    pub removed: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            enabled: true,
            errors: 0,
            last_error: String::new(),
            removed: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginStates {
    #[serde(default)]
    pub plugins: std::collections::BTreeMap<String, PluginState>,
}

impl PluginStates {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<PluginStates>(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, serde_json::to_vec_pretty(self).unwrap_or_default())
            .map_err(|error| format!("保存插件状态失败：{error}"))
    }

    pub fn state(&self, id: &str) -> PluginState {
        self.plugins.get(id).cloned().unwrap_or_default()
    }

    pub fn get_mut(&mut self, id: &str) -> &mut PluginState {
        self.plugins.entry(id.to_string()).or_default()
    }
}

/// 插件列表里给前端看的形状（serde 序列化给启动页）。
#[derive(Clone, Debug, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub mount: String,
    pub builtin: bool,
    pub enabled: bool,
    pub errors: u32,
    pub last_error: String,
    pub path: String,
}

impl PluginInfo {
    pub fn from_plugin(plugin: &Plugin, state: &PluginState) -> Self {
        Self {
            id: plugin.id.clone(),
            name: plugin.meta.name.clone(),
            version: plugin.meta.version.clone(),

            description: plugin.meta.description.clone(),
            permissions: plugin
                .meta
                .permissions
                .iter()
                .map(|permission| permission.as_str().to_string())
                .collect(),
            mount: plugin.meta.mount.as_str().to_string(),
            builtin: plugin.builtin,
            enabled: state.enabled,
            errors: state.errors,
            last_error: state.last_error.clone(),
            path: plugin
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_metadata() {
        let source = r#"
// @name        护眼滤镜
// @version     1.2.0
// @description 给工具箱加一层暖色滤镜
// @permissions dom, ui
// @mount       overlay
const x = 1
"#;
        let meta = parse_meta(source, "fallback");
        assert_eq!(meta.name, "护眼滤镜");
        assert_eq!(meta.version, "1.2.0");
        assert_eq!(meta.mount, Mount::Overlay);
        assert!(meta.has(Permission::Dom));
        assert!(meta.has(Permission::Ui));
        assert!(!meta.has(Permission::Network));
        assert_eq!(meta.permissions, vec![Permission::Dom, Permission::Ui]);
    }

    #[test]
    fn stops_at_first_code_line() {
        let source = "// @name A\nconst code = 1\n// @name B\n";
        assert_eq!(parse_meta(source, "x").name, "A");
    }

    #[test]
    fn unknown_permissions_are_ignored_and_mount_defaults_to_none() {
        let meta = parse_meta("// @permissions dom, telepathy\n", "p");
        assert_eq!(meta.permissions, vec![Permission::Dom]);
        assert_eq!(meta.mount, Mount::None);
        assert_eq!(meta.name, "p");
    }

    #[test]
    fn scan_skips_templates_and_sorts() {
        let root = std::env::temp_dir().join(format!(
            "coconut-plugins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("b.js"), "// @name B\n").expect("write");
        std::fs::write(root.join("a.js"), "// @name A\n").expect("write");
        std::fs::write(root.join("_template.js"), "// @name T\n").expect("write");
        std::fs::write(root.join("notes.txt"), "x").expect("write");
        let plugins = scan_plugins(&root);
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].id, "a");
        assert_eq!(plugins[1].meta.name, "B");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quickjs_runs_and_exposes_host_functions() {
        use rquickjs::{Context, Function, Object, Runtime};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        context.with(|ctx| {
            let value: i32 = ctx.eval("1 + 2 * 3").expect("eval");
            assert_eq!(value, 7);

            let host = Object::new(ctx.clone()).expect("object");
            host.set(
                "log",
                Function::new(ctx.clone(), move |message: String| {
                    assert_eq!(message, "hi");
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .expect("function"),
            )
            .expect("set log");
            ctx.globals().set("host", host).expect("set host");
            let out: String = ctx.eval("host.log('hi'); 'ok'").expect("eval host");
            assert_eq!(out, "ok");
        });
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn states_roundtrip() {
        let root = std::env::temp_dir().join(format!(
            "coconut-pstate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("mkdir");
        let path = root.join("state.json");
        let mut states = PluginStates::default();
        states.get_mut("free-models").enabled = false;
        states.get_mut("free-models").errors = 2;
        states.save(&path).expect("save");
        let loaded = PluginStates::load(&path);
        assert!(!loaded.state("free-models").enabled);
        assert_eq!(loaded.state("free-models").errors, 2);
        assert!(loaded.state("missing").enabled);
        let _ = std::fs::remove_dir_all(&root);
    }
}
