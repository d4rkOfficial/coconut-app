# 椰果工具箱启动器插件开发指南

本文件为`AGENTS.md`，无论是开发者还是AI Agent都能阅读的插件编写规则。类型注解看[coconut-plugin.d.ts](./coconut-plugin.d.ts)。

## 插件运行原理
椰果工具箱提供了WebSocket接管接口，启动器为静态的椰果工具箱开启服务，提供QuickJS环境用来运行插件，而插件可走WebSocket接管，从而可以接管其UI界面并执行工具调用。

## 快速开始
创建一个JavaScript文件(.js)，放到启动器插件目录，启动器最多 3 秒内自动把它当成插件加载。

```js
// @name 示例插件
// @version 1.0.0
// @description 这个插件没啥用
// @permissions dom, ui


async function onHello() {

}
```