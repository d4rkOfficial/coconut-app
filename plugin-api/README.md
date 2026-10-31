# plugin-api · 插件 API 类型声明

这里是启动器的**插件 API 上游**：`Coconut` 的类型、工具箱 WS 工具的类型，以及生成脚本。
启动器运行时**不会**读取这个目录（类型声明不随 exe 释放），它是给插件开发用的。

> 写插件请去 **插件开发 SDK** 仓库：
> [`coconut-app-plugin-sdk`](https://github.com/d4rkOfficial/coconut-app-plugin-sdk)
>
> SDK 里已经装好这套类型，并且带模块化示例、esbuild 单文件打包、`deno task dev` 热部署、
> 静态自查（权限 / 头部 / 钩子 / `.dom.js` 自包含）和冒烟测试。
> 本目录的价值是：**它是这些内容的来源**，改 API 时先改这里，再同步到 SDK。

## 文件

| 文件 | 作用 |
| --- | --- |
| `coconut-plugin.d.ts` | `Coconut` 全局对象、钩子（`onStart/onHello/onEvent/onStop`）、权限、事件类型 |
| `tools.d.ts` | 工具箱 143 个 WS 工具的名字 + 入参 JSON Schema（**自动生成，勿手改**） |
| `generate-tools.mjs` | 从工具箱打包产物里抽工具表，重新生成 `tools.d.ts` |
| `deno.json` | 单独用这个目录做类型检查时的配置（`types` 指向上面两个 d.ts） |
| `AGENTS.md` | 插件开发规范原文（同时也是 SDK 仓库 `docs/plugin-api.md` 的来源） |

## 让插件用上类型提示

把整个 `plugin-api/` 目录拷到插件旁边，插件文件开头加一行：

```js
/// <reference path="./plugin-api/coconut-plugin.d.ts" />
// @ts-check
```

然后：

```text
deno check --config plugin-api/deno.json my-plugin.js
```

`noImplicitAny` 是故意关掉的：参数个数 / 类型写错仍会报错，但不强制写满类型注解。

## 工具箱更新后重新生成 tools.d.ts

```text
node plugin-api/generate-tools.mjs                 # 自动找启动器数据目录里的工具箱
node plugin-api/generate-tools.mjs --dir <toolbox-static>
```

生成完把 `tools.d.ts` 一并同步到 SDK 仓库（`coconut-app-plugin-sdk/plugin-api/`），
两边保持一致；`tools.d.ts` 是机器生成的，不要手改。
