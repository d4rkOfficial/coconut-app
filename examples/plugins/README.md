# examples/plugins · 单文件示例

这里只放**最小、可直接拖进插件目录**的单文件示例（用来快速验证「一个 .js 就能跑」）。

- `eye-care.js`：注入页面 CSS + 右下角浮层，`onHello` 注册、`onStop` 收干净。

正式写插件请去插件开发 SDK 仓库：
[`coconut-app-plugin-sdk`](https://github.com/D4rK6/coconut-app-plugin-sdk)

SDK 的 `examples/` 里有更完整的真实插件（模块化 + `page/*.dom.js` 页面脚本 + 打包 / 热部署 / 测试）：

| 示例 | 看点 |
| --- | --- |
| `buff-coverage` | 只读浮层、几何标定、色块过渡动画 |
| `echo-substat-optimizer` | 大量 `callTool` 编排、快照/还原、预算控制 |
| `cjk-scramble` | `runJs` 注入自包含页面脚本 + `MutationObserver` |
| `free-models` | 多文件 + IndexedDB 只读 + 幂等同步 |

插件规范（钩子、权限、运行时边界）见 [`../../plugin-api/AGENTS.md`](../../plugin-api/AGENTS.md)。
