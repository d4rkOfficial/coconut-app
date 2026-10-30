# builtin/ · 内置脚本源码

启动器自带、编译进 exe 的脚本（不是用户插件，但用的是**同一套 `Coconut` API**）。
源码在这里，用 [esbuild](https://esbuild.github.io/) 打包成单文件后才进安装包。

## 目录

```text
builtin/
  build.mjs            esbuild 打包脚本（npm run builtin）
  out/                 产物，被 src-tauri 用 include_str! 编进 exe（**不入库**，打包前生成）
    free-models.js
    translate.js
  free-models/
    index.js           入口（钩子 + 同步逻辑）
    free-models.dom.js 注入页面的小脚本（普通函数，见硬规则 4）
  translate/
    README.md          翻译插件的设计 / 数据源 / 三层替换
    index.js           入口（钩子 + 调度）
    translate.dom.js   注入页面的 DOM 替换逻辑（普通函数）
    inject.js          用 toString() 把 translate.dom.js 的函数序列化进 runJs
    ui.js              界面词表（唯一来源，人工校对）
    …（api / config / gamecopy / glossary / cache）
```

## 用法

```text
npm run builtin            # 打包全部
node builtin/build.mjs translate   # 只打一个
npm run tauri:build        # 先打包内置脚本，再出 MSI
```

`out/` 不提交进仓库（已在 `.gitignore` 里）。所以：

- 改了 `builtin/*/index.js` 之后**必须**先 `npm run builtin`；
- `npm run tauri:build` 会自动先跑一次内置脚本打包；
- 直接 `cargo build` 时如果 `out/` 不存在，会以「找不到文件」编译失败——这是故意的。

产物 `out/<名字>.js` 会被 `src-tauri/src/plugin_host.rs` 里的 `include_str!` 引用，
所以**改完内置脚本必须重新打包再 `tauri build`**。

## 四条硬规则

1. **入口文件末尾必须把钩子挂到 `globalThis`**
   esbuild 会整份代码包进 IIFE，`function onHello() {}` 不再落在全局作用域，
   而插件宿主是在 `globalThis` 上找钩子的：

   ```js
   globalThis.onStart = onStart;
   globalThis.onHello = onHello;
   globalThis.onEvent = onEvent;
   globalThis.onStop = onStop;
   ```

2. **文件开头的 `// @name` 头部要留着**
   minify 会把注释删掉，打包脚本会自己把这些 `// @...` 行抽出来用 `banner` 放回去。
   少 `@name` 会直接报错。

3. **单文件、无 import 运行时**
   QuickJS 里没有 `import` / `require`，只能在**源码**里 `import`（打包时会被内联）。
   产物里不能有 `import`、动态 `import()`、`eval` 依赖的模块解析。

4. **要注入页面的脚本写成普通函数，文件名以 `.dom.js` 结尾**
   放进页面执行的那段代码放在 `*/*.dom.js`，写成**完全自包含**的函数
   （不引用模块作用域里的任何变量），然后：

   ```js
   // 注入（translate 用这个，见 inject.js）
   const code = `return (${String(translatePage).trim()})(${JSON.stringify(payload)});`;
   // 或者 （free-models 用这个）
   const code = `return (${String(pageFn).trim()})();`;
   ```

   `toString()` 拿到的是**压缩后**的源码，所以页面脚本跟插件一起被 minify，
   不用再手写一大段字符串；字典之类的外部数据用参数传进去。
