# plugin-api → 已迁移到插件 SDK 仓库

这个分支只保留跳转链接：启动器源码（`main` 分支）已不再包含 `plugin-api/` 文件夹，
插件 API 的类型声明与工具都统一放在 **插件开发 SDK** 里：

## https://github.com/d4rkOfficial/coconut-app-plugin-sdk

| 想要 | 去哪 |
| --- | --- |
| `coconut-plugin.d.ts`（`Coconut` / 钩子 / 权限类型） | SDK 仓库 `plugin-api/` |
| `tools.d.ts` + `generate-tools.mjs`（工具箱 WS 工具类型与生成脚本） | SDK 仓库 `plugin-api/` |
| 插件开发规则（钩子、权限、运行时边界） | SDK 仓库 `AGENTS.md` |
| API 规范原文 | SDK 仓库 `docs/plugin-api.md` |
| 现成示例 + 打包 / 热部署 / 自查 / 冒烟测试 | SDK 仓库 `examples/`，命令用 `deno task` |

快速开始：

```text
git clone https://github.com/d4rkOfficial/coconut-app-plugin-sdk.git
cd coconut-app-plugin-sdk
deno task dev <插件id>      # 改完保存即热部署到启动器（3 秒内热重载）
```

启动器本体在 [coconut-app](https://github.com/d4rkOfficial/coconut-app) 的 `main` 分支。