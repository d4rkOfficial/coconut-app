# builtin/ · 内置插件源码

启动器自带、用 `include_str!` 编进 exe 的插件源码。启动器第一次运行时会把它们**导出到
`%APPDATA%\com.coconut.launcher\plugins\`**，之后只从插件目录加载：用户改过 / 删过都以目录里的
文件为准；只有目录里没有同名文件时才重新导出。

> 写普通插件请去 [`coconut-app-plugin-sdk`](https://github.com/d4rkOfficial/coconut-app-plugin-sdk)：
> 那里有热部署、类型声明、示例和自查。免费模型已经迁到 SDK 的 `examples/free-models`。

## 当前内置插件

```text
builtin/
  debug-terminal.js    可读写调试终端（插件层 / DOM 层 REPL、console 输出）
  build.mjs            旧的内置脚本打包脚本（当前没有需要打包的目录）
```

`debug-terminal.js` 是**单文件插件**，不需要 esbuild：启动器直接把它复制进插件目录。

## 硬规则

1. 单文件、无 `import` / `require`；入口末尾把钩子挂到 `globalThis`。
2. 文件开头保留 `// @name`、`// @permissions` 等头部元数据，UTF-8 无 BOM。
3. 注入页面的代码必须写成完全自包含的普通函数（只靠参数和页面全局）。
4. 内置插件导出后以插件目录文件为准：更新内置源码不会覆盖用户改过的同名文件。

## 调试终端

- `console.*` 不再写日志文件，只进终端；`Coconut.log` 才写 `logs/plugins.log`。
- `console.clear()` 清空终端。
- 终端从 `http://127.0.0.1:47821/debug/console` 轮询 console 环形缓冲，这是启动器内部通道，
  不是公开插件 API。
- 输入框回车：括号闭合则运行，括号未闭合则换行；`Tab` 在插件层 / DOM 层之间切换。
- `Ctrl+T` / `Ctrl+\`` 开关终端，`Esc` 关闭。
