# 椰果工具箱启动器 · coconut-app

Tauri 桌面启动器：本地打开椰果工具箱，并自动接管 AI 助手与免费模型配置。

现在**运行时不再依赖 Node、git、pnpm**：

- 工具箱静态产物由 <code>adapter-static</code> 构建；
- Tauri 的 Rust 进程直接启动静态文件服务、API 反向代理、免费模型代理和 WebSocket 接管服务；
- 只有构建工具箱静态产物时才需要 Node/pnpm，打包后的启动器不需要它们。

## 相关仓库

- [`wuwa-afyg-tool`](https://github.com/d4rkOfficial/wuwa-afyg-tool) — 工具箱本体（SvelteKit）
- [`wuwa-afyg-share`](https://github.com/d4rkOfficial/wuwa-afyg-share) — 椰果工坊：分享站 +
  [启动器下载页](https://wuwa-afyg-share.200503.xyz/launcher)
- [`coconut-app-plugin-sdk`](https://github.com/d4rkOfficial/coconut-app-plugin-sdk) — 插件开发 SDK
  （示例 + 打包 / 热部署 / 自查 / 测试）

写插件请用 SDK：里面已有完整示例、类型声明和一键热重载，不必照着本文件手写单文件。

---

## 当前架构

~~~text
┌────────────────────────────────────┐
│ Tauri 窗口 / Rust 进程               │
│                                    │
│  ┌─ 47821 ──────────────────────┐  │
│  │ 工具箱静态文件                 │  │
│  │ /api/* -> 官方 API 反向代理     │  │
│  │ /v1/*  -> 免费模型代理          │  │
│  └───────────────────────────────┘  │
│  ┌─ 8765 ───────────────────────┐  │
│  │ WS 接管 / 插件宿主 / 浮层宿主   │  │
│  └───────────────────────────────┘  │
└────────────────────────────────────┘
               │
               ▼
    工具箱子页面（WebView2）
    http://127.0.0.1:47821/#websocket=127.0.0.1:8765
~~~

所有服务都在 Rust 进程内：

- 静态文件服务
- SvelteKit <code>/api/*</code> 反向代理
- OpenAI 兼容免费模型代理
- WebSocket 接管
- AI 配置文件同步、主题透传、默认配置清理
- 窗口卸载 / 浏览器数据清理

---

## 静态产物与 GitHub Release

静态产物不再单独维护 <code>coconut-toolbox-dist</code> 仓库，全部在工具箱源码仓库完成：

~~~text
https://github.com/d4rkOfficial/wuwa-afyg-tool
~~~

push 到 <code>main</code> 后，<code>.github/workflows/release-toolbox.yml</code> 会：

1. 用 <code>DEPLOY_TARGET=static</code> 构建 adapter-static 产物；
2. 生成 <code>build/manifest.json</code>（version / commit / builtAt）；
3. 打包 <code>toolbox.tar.gz</code>；
4. 创建 <code>toolbox-&lt;sha&gt;</code> Release，并上传：

   - <code>manifest.json</code>
   - <code>toolbox.tar.gz</code>

5. 默认保留最近 3 个 Release，更旧的 Release 连同 git tag 自动删除。

启动器检查更新时读取：

~~~text
https://github.com/d4rkOfficial/wuwa-afyg-tool/releases/latest/download/manifest.json
https://github.com/d4rkOfficial/wuwa-afyg-tool/releases/latest/download/toolbox.tar.gz
~~~

> 首次启用时，在仓库 <code>Settings → Actions → General → Workflow permissions</code> 选择 <code>Read and write permissions</code>，否则 workflow 创建 Release 会 403。

启动器内置一份静态产物：

~~~text
coconut-app\src-tauri\toolbox-static     构建时被 build.rs 打进 exe
~~~

<code>dist\</code> 里只有 <code>coconut-app.exe</code> 一个文件（10.29 MB），不再复制一份
<code>toolbox-static</code> —— 静态产物已经内嵌在 exe 里（见「单文件便携」）。
如果确实想在 exe 同级放一份可热替换的静态目录，仍然会被优先使用。

当前内置版本：

~~~text
commit   bf3061e407f9b344f6aa48fa734a95b7c81803ed
短 sha    bf3061e（manifest.json 里就是它）
builtAt  2026-10-20T14:19:35.190Z
文件     129 个 / 7.88 MB
来源     Release toolbox-bf3061e407f9b344f6aa48fa734a95b7c81803ed
~~~

> <code>manifest.json</code> 里存的是**短 sha**：检查更新时是拿它和 GitHub Release 的
> <code>manifest.json</code> 逐字比较的，写全 40 位会让启动器永远认为「有新版本」。
> 全 sha 只作为人工核对记录写在这里。

便携版仍可直接替换 <code>dist/toolbox-static</code>，无需重新编译 Rust。

---

## 启动页

- 黑白灰现代工具风格，红黄绿仅作为状态点 / 细节点缀。
- **沉浸式标题栏**：窗口无系统装饰（<code>decorations: false</code>），没有左上角标题；
  顶部 38px 是拖动区，右上角是绿（最小化）/ 黄（缩放）/ 红（关闭）三个圆点。
- 全局禁用文本选中（<code>user-select: none</code>），输入框单独放开。
- 右侧展示 <code>icon.svg</code> 椰果图标（纯装饰，不再有任何长按交互）。
- 顶部：<code>Wuthering Waves Coconut Toolbox</code> / <code>Vol. 1 — 排轴 / 拉表 / 配装 / 计算</code>。
- 底部：<code>鸣潮社区公益工具</code>。
- 主按钮：启动工具箱。
- 两个检查按钮：检查工具箱版本更新、检查游戏数据版本更新。
  检查结果 3 秒后恢复默认文案，且两个按钮文案不同：
  「工具箱已是最新版本」/「游戏数据已是最新版本」。
- 主页的游戏数据版本号与左侧「游戏数据版本」文字等大、按基线对齐。
- 工具箱「检查版本」与「下载更新」两个阶段都带「太慢了？切换镜像」链接：
  点击后显示「已切换」并禁用，1 秒后恢复；镜像按 直连 → 镜像 1 → 镜像 2 → 直连 轮换。
- **右下角**两个按钮：「插件」+「卸载并删除本地数据」。
  - 「插件」打开插件弹窗（列表 / 启停 / 重载 / 打开目录 / 重新扫描），
    见「插件」一节；免费模型现在是内置插件，开关就在那个弹窗里；
  - 有插件报错时「插件」按钮的小圆点变红，全部停用时变灰；
  - 卸载按钮点击后弹出分步卸载弹窗。
- 跟随系统昼夜主题，并通过 WS 把 <code>theme_mode</code> 透传到工具箱。
- **标题栏左侧的太阳 / 月亮按钮**可以手动切换昼夜：点过之后就不再跟随系统，
  选择会写进 <code>settings.json</code> 的 <code>theme</code>，下次启动照旧，
  并且会通过 WS 的 <code>set_setting theme_mode</code> 同步进工具箱。
- **界面语言跟随操作系统**：简体中文 / 繁體中文 / 日本語 / 한국어，其它语言一律英文。
  判定用 <code>navigator.languages</code>（WebView2 直接给系统语言列表），
  <code>zh-Hant / zh-HK / zh-MO</code>（以及旧的 <code>zh-TW</code>）走繁体，其余 <code>zh-*</code> 走简体；语言标签统一用 <code>zh-Hans</code> / <code>zh-Hant</code>。
  Rust 侧的进度行、卸载步骤也用同一语言（前端 <code>set_language</code> 告诉后端）。
- 弹窗遮罩跟着主题走：**白天白色、夜间黑色**（<code>--overlay</code> 变量）。
- **滚动条**：WebView2 里 <code>::-webkit-scrollbar</code> 不生效、标准 <code>scrollbar-color</code>
  又会带出箭头和轨道，所以原生滚动条整体隐藏（<code>scrollbar-width: none</code>），
  滑块由前端自绘（<code>.scroll-thumb</code>）：5px 圆角、没有轨道和端点，
  指针进入容器 / 焦点落进去 / 正在滚动时才出现，离开 0.9 秒淡出。

固定端口：

| 端口 | 用途 |
| --- | --- |
| 47821 | 工具箱 + API 代理 + 免费模型代理 + <code>/icons</code> |
| 8765 | WS 接管 |
| 47822 | 椰果工坊同站反代（见「工坊反代」） |

固定 47821 是为了让 WebView2 的 localStorage / IndexedDB origin 稳定。

---

## 工具箱沉浸式外壳

工具箱窗口同样没有系统标题栏。「启动工具箱」后，工具箱的 WS 首次握手（<code>hello</code>）
会让启动器调用 <code>run_js</code> 把 <code>src-tauri/src/shell_inject.js</code> 注入页面
（幂等，重连 / 刷新会重新挂载）：

- sidebar 直接顶到窗口边缘，沉浸式全高；
- welcome 页顶部留出一段 padding，高度 = 活动工程顶部工具栏高度
  （<code>--coconut-titlebar-h</code>，实测工具栏实际高度）；
- 这段 padding 与活动工程的顶部工具栏都可以拖动窗口；
- 拖动区只覆盖 sidebar 右侧的内容区：**sidebar 顶部（logo / 回主页入口）不参与拖动**，
  点击照常交给工具箱；
- 顶部工具栏里的「WS 远程接管」「使用帮助」按钮隐藏（启动器已常驻接管）；
- **WS 接管被强制开启、且用户关不掉**：
  - 注入时把 <code>localStorage</code> 的 <code>wuwa-afyg:ws-prefs:enabled</code>
    写成 <code>1</code>、<code>wuwa-afyg:ws-prefs:url</code> 补成 <code>127.0.0.1:8765</code>，
    这样以后刷新 / 重进都会自动重连（不只靠 <code>#websocket=</code> hash）；
  - 设置 → 连接配置 里「WS 远程接管」整块（开关 + 地址 + 状态）被隐藏，用户点不到；
  - 还有一层看门狗：每 0.8 秒检查一次那个开关，一旦被改成 <code>0</code>（别的入口改的、
    旧版本残留、或者用户在设置里抢在隐藏生效前点掉），立刻写回 <code>1</code>
    并把 <code>#websocket=127.0.0.1:8765</code> 重新挂上——工具箱监听 <code>hashchange</code>，
    会当场重连。启动器的日志里能看到重连后的 <code>hello</code>。
- 拖动区右侧只有三个窗口按钮：<code>最小化</code> · <code>缩放窗口</code> · <code>关闭</code>；
- 整体缩放改用快捷键 **<code>Ctrl</code> + <code>-</code> / <code>=</code>**：单击一次 5%，
  按住 0.42 秒后每 110ms 连调（线性），范围 **90% – 120%**；注入脚本在捕获阶段拦截，
  不会透传给工具箱；
- 图标用 <code>mix-blend-mode: difference</code> + 白色描边，自动与下方背景反色；
- 数据上游文案改写：工具箱里写死的 <code>Nanoka (nanoka.cc)</code> / <code>nanoka</code>
  一律显示为 <code>local</code>（数据由启动器本地兜底）；
- Toast 位置改写：设置里是 <code>top-right</code>（或默认）时，强制成 <code>bottom-left</code>
  （右上角会压住窗口控制区），并写回 <code>wuwa-afyg:interaction-prefs:toast-position</code>；
  用户之后自己改成别的值就立即让位；
- 屏蔽「缓存清理」入口：设置 → 数据 里的「缓存清理」导航按钮隐藏（「全部清理 / 清理本类」也一并隐藏）。
  注意这只挡 UI，工具箱内置 AI 仍可用 <code>set_setting clear_cache</code>；
- 图标缓存预热：打开工具箱后读本机 <code>/api/v1/batch-icons/*</code>，把这些
  <code>/icons/*.webp</code> 塞进工具箱自己的 CacheStorage <code>data-cdn</code>，
  避免它再为同一批图发请求（每次会话只做一次，记在 <code>sessionStorage</code>）；
- AI 助手悬浮窗（<code>.ai-assistant</code>）让出标题栏：工具箱全尺寸时高度按
  <code>window.innerHeight - 24</code> 算、位置还能拖到 y≈12，会压住我们的标题栏。
  注入脚本给它加 <code>max-height: calc(100dvh - var(--coconut-titlebar-h) - 16px)</code>，
  并把 <code>top</code> 顶到标题栏下方 8px。
- **不再自动打开 AI 悬浮窗**：以前每次 WS 握手都会 <code>btn.click()</code> 把它展开。
  现在只有在确认工具箱里的 AI 助手是开启状态（或第一次使用）时才打开；
  用户关了 AI 助手，注入脚本连 <code>.ai-assistant</code> 节点都不会去点（此时它压根不渲染）。

> 工具箱的 JSON 缓存（IndexedDB <code>wuwa-afyg-cache</code> / store <code>kv</code>，
> key 形如 <code>wuwa-afyg:v2:list:character</code>）存的是**它自己的数据源**
> <code>https://static.nanoka.cc/ww/&lt;version&gt;/*.json</code> 的原始结构；
> 启动器下载的 <code>/api/v1/*</code> 是另一套结构，直接写进那些 key 会被当成 nanoka 数据解析，
> 所以没有做「把 game-data.json 写进工具箱缓存」。

控制簇通过一条独立 WS 连接向启动器发 <code>{"type":"window","action":...}</code>：

| action | 行为 |
| --- | --- |
| <code>drag</code> | 系统级拖动（<code>start_dragging</code>） |
| <code>zoom-set</code> | 设置整体缩放，启动器夹到 90% – 120% |
| <code>minimize</code> / <code>maximize-toggle</code> / <code>close</code> | 窗口控制 |
| <code>state</code> | 返回当前缩放，页面重载后恢复档位 |

### 工坊反代（47822）

工具箱的「椰果工坊」是一个 <code>&lt;iframe title="椰果工坊"&gt;</code>，指向外站
（默认 <code>https://wuwa-afyg-share.200503.xyz</code>，用户也可以填任意工坊地址）。

直接嵌入时 iframe 的文档是**外站**，而外层页面是 <code>http://127.0.0.1:47821</code>：
两者不同站，外站下发的 <code>SameSite=Lax/Strict</code> 会话 cookie 在 iframe 里
会被浏览器丢掉，于是每刷新一次工坊就退出登录。

启动器在 **47822** 起一条同站反代（同主机、仅端口不同 → **同 site**，cookie 照常生效）：

~~~text
iframe src = http://127.0.0.1:47822/<原 path>?<原 query>&__coconut_target=<原 origin>
~~~

- 注入脚本 <code>rewriteWorkshop()</code> 扫描 <code>iframe[title="椰果工坊"]</code>，
  把任意 <code>http(s)://</code> 外站 src 改写成上面的形式（已是 47822 / 本地地址的跳过，幂等）；
- 反代用 <code>?__coconut_target=</code> 记住上游 origin（<code>state.workshop_target</code>），
  之后该 iframe 发出的绝对路径请求（<code>/login</code>、静态资源、XHR）都沿用它；
- 请求头 <code>origin</code> / <code>referer</code> 改写成上游自己的 origin，
  避免上游 CSRF 校验失败；<code>host</code> 交给 reqwest 按 URL 重算；
- 响应：<code>set-cookie</code> 去掉 <code>Domain=</code> 之后原样转发（<code>Secure</code> 保留，
  127.0.0.1 属于 trustworthy origin），<code>location</code> 里的上游 origin 换回
  <code>http://127.0.0.1:47822</code>，并剥掉 <code>content-length</code> / CSP /
  <code>strict-transport-security</code> / <code>x-frame-options</code>，body 以流式转发；
- 不依赖工坊源码，工坊作者发版不用同步改启动器。

> 端口被占用时启动不会失败，只在日志里提示「无法监听工坊反代端口 47822」，
> 此时工坊退回直连（登录态可能丢失，其余功能正常）。

---

## 运行

~~~text
dist\coconut-app.exe
~~~

需要：

- Windows WebView2 Runtime
- 无需 Node / git / pnpm
- 游戏数据未缓存时，<code>/api/*</code> 反代到官方 API，需要网络
- 点「检查游戏数据版本更新」可把 API 数据与图标拉到本地（先增量后全量），之后优先读本地文件
- 拉取期间可以直接启动工具箱，数据在后台继续拉取
- 免费模型和 AI 助手仍然需要网络

### 单文件便携

<code>coconut-app.exe</code> 把工具箱静态产物（<code>toolbox.tar.gz</code>）**编进了二进制**：
<code>build.rs</code> 在构建时把 <code>toolbox-static/</code> 打成一个 gzip 包放进 <code>OUT_DIR</code>，
运行时用 <code>include_bytes!</code> 读取。exe 因此从 ~6 MB 变成 ~10.8 MB。

首次启动时静态目录按这个顺序准备（任何一步成功就停）：

1. <code>%APPDATA%\com.coconut.launcher\toolbox-static\index.html</code> 已存在 → 直接用；
2. 安装版：<code>resource_dir()/toolbox-static</code> 或 <code>exe 同级\toolbox-static</code>
   （MSI 安装、便携目录都走这里）；
3. 单文件：把内嵌的压缩包解到数据目录；
4. 以上都不行 → 建一个空目录，**启动页提示「本地缺少工具箱静态文件，请点「检查更新」下载」**，
   不再在 setup 阶段 panic 闪退。

> 0.1.0 的旧构建在第 4 步是直接 <code>return Err</code>，Tauri 会把 setup 失败升级成
> <code>panic → 0xC0000409</code> 直接退出，用户连报错窗口都看不到（单文件运行时 100% 复现）。
> 现在这条路径改成降级提示，且 <code>release</code> 构建不再把开发机的
> <code>CARGO_MANIFEST_DIR\toolbox-static</code> 当兜底。

### MSI 体积

静态产物只存一份（内嵌在 exe 里），所以 <code>tauri.conf.json</code> 的
<code>bundle.resources</code> **去掉了** <code>toolbox-static/**/*</code>：

| 配置 | MSI 大小 |
| --- | --- |
| 内嵌 + <code>resources</code> 再装一份（重复） | 12.52 MiB |
| 只有内嵌（当前） | **7.92 MiB** |

MSI 里现在只有一个文件条目（<code>coconut-app.exe</code>）；首次启动时由 exe 自己把
内嵌的静态包解到 <code>%APPDATA%</code>。代价是安装目录里没有可热替换的
<code>toolbox-static</code>（需要时手动放一个同目录即可，优先级高于内嵌）。

---

## 构建

前端启动页是原生 HTML/CSS/JS，无需构建。Rust/Tauri：

~~~bash
pnpm install
node node_modules/@tauri-apps/cli/tauri.js build
~~~

如果使用 <code>pnpm dev</code> 遇到 Tauri CLI 参数错误，直接使用上面的 <code>node ... tauri.js</code>。

### MSI 安装包外观

安装界面用了自定义位图（WiX 标准尺寸，24 位 BMP）：

| 文件 | 尺寸 | 用在哪 |
| --- | --- | --- |
| <code>assets/msi/banner.bmp</code> | 493 × 58 | 各安装页顶部的横幅 |
| <code>assets/msi/dialog.bmp</code> | 493 × 312 | 欢迎页 / 完成页 / 准备页的整块背景 |
| <code>assets/msi/render/</code> | — | 两张图的 HTML/CSS 源，改完重新渲染即可 |

**排版必须避开 WiX 自己画的文字**（MSI 控件几何，1 unit = 1/72 inch，96 DPI 下 ×4/3）：

- 横幅 370 × 44 units = 493 × 58 px；页面标题在左侧 <code>x 15..215 units（20..287 px）</code>，
  所以**品牌只能放右侧**，左侧留空；
- 侧图 370 × 234 units = 493 × 312 px；欢迎页 / 完成页的标题与说明从
  <code>x = 135 units（180 px）</code> 开始，所以**品牌只能放左侧 176 px 的竖栏**。

第一版没注意这两点，结果横幅的 logo 被页面标题压住、侧图的大标题跟安装程序正文重叠。

渲染 → 转 BMP：

~~~powershell
# 无头 Edge 按精确像素渲染（否则视口比图矮会裁掉底部）
msedge --headless --force-device-scale-factor=1 --remote-debugging-port=9223 --window-size=700,500 "^
  file:///<repo>/assets/msi/render/banner.html"
# CDP: Emulation.setDeviceMetricsOverride(493x58 / 493x312) → Page.captureScreenshot
# 再用 System.Drawing 以 Format24bppRgb 存成 .bmp
~~~

在 <code>tauri.conf.json</code> 里接上：

~~~json
"bundle": {
  "publisher": "椰果工具箱",
  "copyright": "© 2026 椰果工具箱 · 鸣潮社区公益工具",
  "windows": {
    "wix": {
      "language": "zh-Hans",
      "bannerPath": "../assets/msi/banner.bmp",
      "dialogImagePath": "../assets/msi/dialog.bmp"
    }
  }
}
~~~

图是先在无头 Edge 里按精确像素渲染 PNG，再转成 24bpp BMP 的（细节见上一节）。

> <code>publisher</code> 不设时默认取 identifier 的第二段（<code>coconut</code>），
> 「应用和功能」里会很难看，所以显式写成「椰果工具箱」。

<code>productName</code> 也改成了「椰果工具箱」：它决定「应用和功能」里的显示名、
安装目录（<code>INSTALLDIR = 椰果工具箱</code>）和 MSI 文件名；exe 仍是
<code>coconut-app.exe</code>（由 Cargo 包名决定），所以 <code>dist</code> 的便携部署方式不受影响。

产物：

~~~text
src-tauri\target\release\bundle\msi\椰果工具箱_0.1.0_x64_zh-CN.msi
~~~


工具箱静态产物在 <code>wuwa-afyg-tool</code> 仓库构建并自动发布 Release：

~~~bash
DEPLOY_TARGET=static pnpm run build
node scripts/write-static-manifest.mjs
tar -czf toolbox.tar.gz -C build .
~~~

push 到 <code>main</code> 后由 <code>.github/workflows/release-toolbox.yml</code> 自动完成，无需再同步到独立仓库。

如果要更新启动器内置的兜底产物，把 <code>build/</code> 内容同步到：

- <code>coconut-app/src-tauri/toolbox-static/</code>

<code>manifest.json</code> 跟着一起换。重新 <code>tauri build</code> 后 <code>build.rs</code>
会自动把这份目录重新打进 exe（<code>dist\</code> 里不需要再放一份）。

---

## 本地数据

工具箱固定运行在：

~~~text
http://127.0.0.1:47821
~~~

WebView2 数据位于：

~~~text
%LOCALAPPDATA%\com.coconut.launcher\EBWebView\
  Default\Local Storage\
  Default\IndexedDB\http_127.0.0.1_47821.indexeddb.leveldb\
  Default\Service Worker\
~~~

启动器配置 / 日志位于：

~~~text
%APPDATA%\com.coconut.launcher\
  game-data.json           本地游戏数据缓存
  free-models.json         免费模型列表缓存（含 fetchedAt，24 小时有效）
  opencode-key.json        用户填的 opencode API Key
  settings.json            启动器自己的开关（language / theme）
  toolbox-static\          工具箱静态产物（自动更新会替换这里）
  icons\                   本地化后的游戏图标（webp）
  service.log              启动器运行日志
  logs\plugins.log         插件日志（每次打开工具箱都会换新的一份）
  logs\plugins-<时间>.log   上一次工具箱会话的日志备份（只留最近 10 份）
~~~

<code>settings.json</code> 内容示例：

~~~json
{
  "language": "zh-Hans",
  "theme": "dark"
}
~~~

- <code>language</code>：启动页语言（<code>zh-Hans</code> / <code>zh-Hant</code> / <code>ja</code> / <code>ko</code> / <code>en</code>，旧的 <code>zh-CN</code> / <code>zh-TW</code> 仍识别）；
- <code>theme</code>：昼夜主题（<code>light</code> / <code>dark</code>），空串表示还没选过；
- 免费模型的启用状态、以及「是否已写过一次」都挪进了插件体系
  （<code>plugins/state.json</code> 与 <code>plugins/storage/free-models.json</code>）。

右下角「卸载并删除本地数据」会打开分步弹窗，逐步显示并执行：

1. **停止本地服务** —— 断开 WS 接管，停止静态服务与 API / 模型代理；
2. **清除浏览器存储** —— localStorage / IndexedDB / Service Worker；
3. **删除工具箱静态文件** —— <code>toolbox-static</code>；
4. **删除游戏数据缓存** —— <code>game-data.json</code>；
5. **删除免费模型缓存与 API Key** —— <code>free-models.json</code> / <code>opencode-key.json</code>；
6. **删除 WebView2 用户数据** —— <code>%LOCALAPPDATA%</code> 下的缓存 / Cookie /
   Local Storage / IndexedDB / Crashpad；
7. **退出启动器** —— 清理完成后**不会立刻退出**：弹窗显示剩余时间与「退出启动器」
   按钮，点按钮立即退出；不理它则静默等待 **5 分钟**后自动退出。
   退出时才挂上脱离父进程的 Powershell 清理任务，等 WebView2 释放
   <code>lockfile</code> 后再重试删除，最多 30 秒。

弹窗底部是红黄绿渐变的进度条，全部完成时显示 100%。

> WebView2 在进程运行期间会锁住 <code>EBWebView</code>，所以真正的彻底删除只能在
> 退出之后完成；任务管理器里若残留 <code>msedgewebview2.exe</code>，清理任务会等到
> 它们退出为止。安装版（MSI）卸载还需要走系统「应用和功能」，启动器内按钮只负责
> 清理本地数据。

> 工具键盘数据、工程数据、AI 配置都保存在浏览器 IndexedDB / localStorage 中，卸载不可恢复。

---

## 插件

内置插件（免费模型、翻译）和用户插件走同一套 API，源码在 `builtin/` 下，
用 esbuild 打包成单文件后编进 exe。插件系统在启动器进程内用
[rquickjs](https://github.com/DelSkayn/rquickjs)（QuickJS）跑 JS，插件通过
<code>Coconut.runJs</code> / <code>Coconut.callTool</code> 操作工具箱页面。

### 放哪、长什么样

~~~text
%APPDATA%\com.coconut.launcher\plugins\
  eye-care.js          用户插件：拖一个 .js 进来就能用
  state.json           每个插件的 enabled / errors / lastError
  storage\<id>.json    插件私有 KV（Coconut.storage）
~~~

插件类型声明（`coconut-plugin.d.ts` + 生成好的 `tools.d.ts`）和 `generate-tools.mjs` **不在本仓库**，
统一放在插件开发 SDK 里：

- 仓库：[`coconut-app-plugin-sdk`](https://github.com/d4rkOfficial/coconut-app-plugin-sdk)
- 类型与生成脚本：SDK 的 `plugin-api/`
- 开发规则 / API 原文：SDK 的 `AGENTS.md` / `docs/plugin-api.md`

`tools.d.ts` 是**根据工具箱包体生成**的工具表（143 个工具的名字 + 入参 JSON Schema），
`deno check` 时 `Coconut.callTool` 能补全工具名、校验参数。工具箱更新后在 SDK 仓库里重新生成：

~~~text
deno task tools
~~~

> 写插件请直接用 [SDK 仓库](https://github.com/d4rkOfficial/coconut-app-plugin-sdk)：类型声明、esbuild
> 打包、热部署、自查与示例都已配好（`deno task dev <插件id>`）。

一个插件就是一个文件，开头用注释声明元数据：

~~~js
// @name        护眼滤镜
// @version     1.0.0
// @description 给工具箱加一层暖色滤镜，并显示一个小浮层
// @permissions dom, ui
// @mount       overlay

async function onHello() {
  Coconut.ui.injectCss('eye-care', 'html { filter: sepia(0.22) !important; }');
  const id = Coconut.ui.overlay({ html: '<b>滤镜已启用</b>' });
  await Coconut.sleep(3000);
  Coconut.ui.hide(id);
}
~~~

- <code>@mount</code>：<code>overlay</code> = 用启动器提供的浮层容器；
  <code>none</code> = 只自己写 DOM；
- 目录里以 <code>_</code> 开头的文件会被跳过（用来放模板）；
- 文件带 BOM 也没关系（会先剥掉）；拖进目录最多 3 秒自动加载。

### 权限

| 权限 | 提供什么 | 说明 |
| --- | --- | --- |
| <code>dom</code> | <code>Coconut.runJs</code> | 在工具箱页面里执行 JS 并等返回值 |
| <code>toolbox</code> | <code>Coconut.callTool</code> | 调用工具箱任意 WS 工具（<code>manage_ai_profile</code> 等） |
| <code>network</code> | <code>Coconut.fetch</code> | 走启动器 reqwest 发 HTTP，不限域名 |
| <code>storage</code> | <code>Coconut.storage</code> | 插件私有 KV，落盘在 <code>plugins/storage/&lt;id&gt;.json</code> |
| <code>ui</code> | <code>Coconut.ui</code> | 浮层（注册 / 显隐 / 移除）+ 页面 CSS 注入（需要同时有 <code>dom</code>） |
| <code>debug</code> | <code>onEvent</code> 里的原始帧 | 只读订阅 WS 收发帧（调试用） |

权限在 Rust 侧强制：没声明的 API 直接抛错，不是靠文档约束。启动页「插件」弹窗里
每个插件都会列出它要的权限。

### 生命周期与 API

| 钩子 | 时机 |
| --- | --- |
| <code>onStart()</code> | 插件线程起来（工具箱页面可能还没打开） |
| <code>onHello(info)</code> | 工具箱页面就绪（<code>info.session</code> / <code>info.tools</code>） |
| <code>onEvent(event)</code> | 结构化事件（<code>resync</code>、<code>debug</code> 下的原始帧） |
| <code>onStop()</code> | 停用 / 重载 / 退出 |

钩子可以写成 <code>async function</code>；<code>await</code> 一个同步 API 也没问题
（宿主调用是同步的，只多走一次微任务）。可用 API：

~~~js
Coconut.plugin            // { id, name, version, mount, permissions }
Coconut.runJs(code)       // 代码是「函数体」（工具箱自己会包 async IIFE，别再套 IIFE）
Coconut.callTool(name, args)
Coconut.fetch(url, { method, headers, body | json })
Coconut.storage.get(key, fallback) / .set(key, value) / .delete(key) / .all()
Coconut.ui.overlay({ html, css })  // → 浮层 id（引擎分配 z-index，后注册的在上）
Coconut.ui.visible(id, true|false) / .show(id) / .hide(id) / .remove(id)
Coconut.ui.injectCss(id, css)
Coconut.launcher                  // { lang, toolboxVersion, gameVersion }
Coconut.src.toolbox / .static     // { dir（本地路径）, url（本地服务地址） }
Coconut.sleep(ms)                 // await Coconut.sleep(500)
Coconut.log(...args)      // 写到 logs\plugins.log；工具箱里 Ctrl+T 可以看（只读）
Coconut.setTimeout / Coconut.setInterval / Coconut.clearTimeout / Coconut.clearInterval
~~~

- 单个钩子超过 **10 秒**会被中断（QuickJS interrupt handler）；
- 连续 **3 次**抛错自动停用，列表里标红并显示最后一次错误，用户可手动重新启用；
- 插件线程互不影响：某个插件死循环不会拖住启动器（release 构建已改成
  <code>panic = "unwind"</code>，并把 panic 位置写进
  <code>%APPDATA%\com.coconut.launcher\panic.log</code>）。

### 启动页「插件」弹窗

右下角「插件」按钮打开，右上角 ✕ / `Esc` 关闭：

- 标题栏：插件数 + 启用数（有报错转红），滚动时固定在顶部；右侧是打开目录 / 重新扫描；
- 工具条：按名称 / id / 描述 / 权限搜索，按「全部 / 已启用 / 已停用 / 异常」筛选（带数量），
  一键全部启用 / 停用；
- 每行：状态点 + 名称 / 版本 / 徽标 + 描述 + 权限标签 + 错误信息（悬停看全），
  右侧是启用开关与重载；
- **启动器里不提供删除**（手动进插件目录删；内置插件没有文件，只能停用）；
  日志在工具箱里按 `Ctrl+T` 看，弹窗内没有日志入口；
- 免费模型的开关就是内置插件「免费模型」的启用状态。

### 内置插件：免费模型 / 翻译

```text
builtin/
  free-models/index.js     免费模型
  translate/               页面翻译（config / packs / ui / cache / inject 多文件）
out/                       产物（不入库）
```

`npm run builtin` 打包，`npm run tauri:build` 会自动先跑一次。

- **免费模型**：把启动器免费模型代理里的模型同步成工具箱的 AI 配置；
- **翻译**：按启动器语言把界面翻译成对应语言，游戏数据按 id 从
  `https://static.nanoka.cc/ww/<版本>/` 取译文，首次用到某语言（或游戏版本变化）时
  拉取一次并缓存进 `plugins/storage/translate.json`，之后零请求。

### 内置插件：免费模型的实现

<code>builtin/free-models/index.js</code>（esbuild 打包成 <code>builtin/out/free-models.js</code> 后编进 exe），用和用户插件
**完全一样的 API** 完成原来 Rust 里的配置同步：

- 备注名统一 <code>OC_FREE</code>（<code>\u200BOC_FREE\u200C</code>），模型名在
  <code>model</code> 字段，因此「是不是同一条配置」按 <code>model</code> 判断；
- 模型列表来自启动器自己的 <code>/v1/models</code>（那边一天只向上游拉一次）；
- 走 <code>Coconut.callTool('manage_ai_profile')</code> 新增 / 更新 / 删除配置，
  同一模型重复的会被去重删掉；
- 读工具箱 <code>ai-gen-prefs</code>（<code>Coconut.runJs</code>）判断用户有没有关 AI 助手：
  关了就把免费模型全删掉，并且**不打开 AI 悬浮窗、不强行打开 <code>ai_enabled</code>**；
- 第一次使用（<code>Coconut.storage.seeded</code> 未置位、或工具箱还没有偏好记录）会写入
  模型并打开 AI 助手——工具箱首次进入会自己把 AI 写成关闭，不能当成用户的选择；
- 页面刚起来、会话还没就绪时，插件自己按 3 秒 × 6 次重试，不打扰用户。

> 代理本身（<code>/v1/models</code>、<code>/v1/chat/completions</code>）仍在 Rust 里，
> 插件只负责「把这些模型同步进工具箱的 AI 配置」。

示例插件见 <code>examples/plugins/eye-care.js</code>。

---
## 免费模型代理

Rust 实现，模拟 opencode 免费网关握手：

- <code>User-Agent: opencode/1.19.3</code>
- <code>x-opencode-session</code> + <code>x-opencode-session-id</code>
- <code>Authorization: Bearer &lt;key&gt;</code>（见下节）
- 强制 <code>stream: true</code>
- 自动补 <code>read</code> 工具，并剥离桥接 read 调用
- 免费模型失败自动降级

上游：<code>https://opencode.ai/inference/v1</code>。

### 免费模型列表

- **一天只向上游请求一次** <code>/models</code>，结果写到
  <code>%APPDATA%\com.coconut.launcher\free-models.json</code>，
  24 小时内重启启动器也直接读缓存，不再握手（避免触发风控）。
- 抓取失败时沿用上次列表；冷启动没有缓存则每 10 分钟重试一次。
- 只保留上游以 <code>-free</code> 结尾的模型，并且：
  - <code>big-pickle</code> 不对外暴露；
  - <code>jev-*</code> 系列不上架。
- **对外隐藏 <code>-free</code> 后缀**：调用方看到的是
  <code>deepseek-v4.1-flash</code>，转发时自动补回
  <code>deepseek-v4.1-flash-free</code>。
  所以工具箱里的模型名、<code>/v1/models</code> 返回值都不带后缀。

### opencode API Key

> 启动页上那个「长按椰果图标」的输入框已经删掉了：现在没有手动填 Key 的入口，
> 代理会一直用 <code>Bearer public</code>（公开额度）。文件格式仍然保留，
> 手动放一份 <code>opencode-key.json</code> 进去照样生效。

- ~~启动页长按椰果图标弹出输入框~~（已移除）；
- Key 保存在 <code>%APPDATA%\com.coconut.launcher\opencode-key.json</code>（仅本机）；
- WS 初始化同步 AI 配置时，把 Key 写进每个托管配置的 <code>apiKey</code>；
- 代理转发规则：
  - 调用方带来的 Key 格式合法 → <code>Authorization: Bearer &lt;该 Key&gt;</code>；
  - 未带 Key 或格式非法 → 用启动器里保存的 Key；
  - 两者都没有 → 退回 <code>Bearer public</code>（公开额度）。
- 合法格式：长度 8–200，仅 <code>[A-Za-z0-9._~+/=-]</code>，
  且不是 <code>public</code> / <code>undefined</code> / <code>null</code> / <code>none</code>。


---

## 游戏数据本地化

「检查游戏数据版本更新」按钮：

1. 请求上游 <code>/api/v1/version/latest</code>，与本地缓存的版本比较；
2. 发现新版本时按钮变为 <code>更新到 v3.7</code>；
3. 再次点击开始拉取，**优先增量、再做全量**：

   - 先拉「清单类」：<code>/api/v1/version/latest</code> / <code>available</code>、
     character / weapon / echo / echo-set 列表、<code>/api/v1/batch-icons/*</code>；
   - 版本与本地一致 → **增量**：只补本地缺失的实体路径
     （<code>/api/v1/info/…</code>、角色的 v2/v3 详情、<code>/recommend-weapon/…</code>），
     已有条目直接沿用；缺口为 0 时一条都不拉；
   - 版本变化或本地无缓存 → **全量**：重新拉取所有实体条目；
4. 图标本地化：把 <code>batch-icons</code> 里的远程图片下载下来，解码后**重新编码成
   无损 webp** 落盘，再把 JSON 里的地址改成本机
   <code>http://127.0.0.1:47821/icons/&lt;hash&gt;.webp</code>（已存在则跳过下载）；
5. 写入：

~~~text
%APPDATA%\com.coconut.launcher\game-data.json    API JSON 缓存
%APPDATA%\com.coconut.launcher\icons\*.webp      本地图标
~~~

之后本地 Rust 服务的 <code>/api/*</code> 优先返回本地条目，<code>/icons/*</code> 直接读磁盘，
本地没有的 <code>/api/*</code> 路径才回源上游。

> 拉取期间**可以直接启动工具箱**：只要本地有工具箱静态产物，启动按钮保持可用，
> 数据在后台继续拉取，完成后再写盘。

---

## 检查更新

### 工具箱版本

**启动器一启动就会自动检查并更新工具箱**（启动 1.5 秒后后台开始）：

- 有新版本 → 自动下载、替换本地静态产物，启动页进度行照常显示下载进度；
- 已是最新 → 什么都不做，只写一行日志；
- 网络不可用 / 下载失败 → 静默跳过（日志里记一条），启动页仍可手动「检查更新」；
- 替换完成后如果工具箱页面已经打开，会直接 <code>location.reload()</code> 刷新，
  用户不用按 Ctrl+Shift+R；
- 自动更新和手动更新互斥（同一个静态目录不能同时替换两次），重复触发会返回
  「工具箱正在更新中」。

手动点「检查工具箱版本更新」时会读取工具箱仓库最新 Release 的 <code>manifest.json</code>：

~~~text
https://github.com/d4rkOfficial/wuwa-afyg-tool/releases/latest/download/manifest.json
~~~

- 本地 commit 与远端 commit 相同：按钮显示「工具箱已是最新版本」3 秒；
- 不同：按钮显示「更新到 &lt;commit&gt;」；
- 再次点击下载同一 Release 的 <code>toolbox.tar.gz</code>，解压替换本地静态产物；
- **检查阶段和下载阶段**的进度行都带「太慢了？切换镜像」链接，点击立刻放弃当前连接、
  改用下一个源（直连 → 镜像 1 → 镜像 2 → 直连），检查时清单同样走镜像前缀；
- 下载只设「30 秒收不到数据就换源」的空闲超时（整体超时会把慢速但活着的下载误杀，
  而不设上限时直连 GitHub 资产经常挂在半路，自动更新就永远完不成）；
- 检查失败 / 下载失败 / GitHub 不可达 / 尚无 Release：按钮显示「检查失败」3 秒。

### 为什么不用 Ctrl+Shift+R

工具箱是 PWA，自带 Workbox Service Worker（<code>sw.js</code>），会预缓存
<code>/</code> 和 <code>_app</code> 资源。静态包更新了，页面还是被 SW 喂旧版本，
所以以前必须硬刷新。启动器现在从两端一起治：

- **缓存头**（47821 静态服务）：<code>_app/immutable/*</code> 和 <code>/icons/*</code>
  文件名带哈希 → <code>public, max-age=31536000, immutable</code>；
  其余（<code>index.html</code> / <code>200.html</code> / <code>manifest.json</code>…）
  一律 <code>no-cache</code>，每次进工具箱都回源校验；
- **<code>sw.js</code> 空壳**：启动器接管工具箱的 <code>/sw.js</code> 与
  <code>/registerSW.js</code>，返回一个不注册 fetch 处理器的空壳 SW：
  activate 时清掉所有 Workbox 预缓存（保留工具箱自己的图片缓存 <code>data-cdn</code>），
  只有真的清到了旧缓存才 <code>client.navigate()</code> 触发一次自动刷新。
  静态文件本来就在本机，SW 在这里只剩「缓存旧版本」这一个作用。

> Release 由 <code>wuwa-afyg-tool</code> 的 <code>release-toolbox.yml</code> 自动创建，不再需要独立的静态产物仓库。仓库 owner/name 变化时修改 <code>src-tauri/src/server.rs</code> 里的 DIST 常量。

### 游戏数据版本

按钮状态：

- 检查中：<code>检查游戏数据版本更新中...</code>
- 失败：<code>检查失败</code>（3 秒后恢复）
- 已最新：<code>游戏数据已是最新版本</code>（3 秒后恢复）
- 可更新：<code>更新到 v3.7</code>
- 更新中：<code>游戏数据更新中...</code>

更新流程会先停止刷新主页状态，避免按钮文字被轮询覆盖。

---

## 目录

~~~text
coconut-app/
  assets/icon.svg
  assets/msi/               安装界面位图（banner/dialog）+ render 源
  ui/                       启动页（i18n.js 跟随系统语言）
  dist/                     便携发布目录
    coconut-app.exe         单文件即可运行（静态产物内嵌）
  src-tauri/
    src/main.rs             Tauri 壳 / 命令 / 生命周期
    src/server.rs           Rust 静态服务 + 本地游戏数据 + 免费代理 + WS + 更新 + 插件宿主桥
    src/i18n.rs             启动页文案本地化（5 种语言）
    src/plugin.rs           插件元数据解析 / 状态落盘
    src/plugin_rt.rs        QuickJS 运行时（钩子 / 超时 / 定时器 / 宿主 API）
    src/plugin_host.rs      插件管理器（目录扫描 / 启停 / 日志 / 错误停用）
    builtin/out/*.js           内置插件（esbuild 产物，编译进 exe）
    src/shell_inject.js     沉浸式外壳 + 插件浮层宿主
    build.rs                把 toolbox-static 打成内嵌压缩包
    toolbox-static/         构建时打包的静态产物
    tauri.conf.json
    Cargo.toml
  examples/plugins/eye-care.js    插件示例（护眼滤镜）
  package.json
  README.md

wuwa-afyg-tool/
  .github/workflows/release-toolbox.yml
  scripts/write-static-manifest.mjs
  build/                     DEPLOY_TARGET=static 产物
~~~

---

## 依赖

运行：WebView2。

构建 Rust：Rust stable、Visual Studio Build Tools、WebView2 SDK（Tauri 自动处理）。

构建静态工具箱：Node/pnpm（只在发布静态产物时使用，不进入最终启动器）。
