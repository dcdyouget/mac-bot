# Mac Bot 集成进度与使用说明

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 现在的状态

截至 `72d4353`，协议 Rust crate、35 个 JSON schema（另有 `.gitkeep`，目录共 36 项）、对象/消息块/事件/轨迹 fixtures 和登录场景已发布。桌面端已从 main 首次打包并安装到 `~/Applications/MacBot.app`，可以打开连接页；正式服务 `7788`、Android APK 和 mock 驱动的完整 S0 场景仍待发布，因此 S0 还未通过联调验收。Android 演示环境已统一为 `macbot_api36`（Android 16 / API 36 / arm64）。

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 桌面 main 预览可打开；正式 7788、Android APK、mock bootstrap 和两端截图待完成 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未验收 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未验收 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未验收 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未验收 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。最新结果和遗留问题以根目录 `COORDINATION.md` 为准。契约发布核查见 [contracts-published.json](S0/contracts-published.json)。历史初次预检见 [verification.json](S0/verification.json)。最新桌面部署核查见 [main-desktop-preview.json](S0/main-desktop-preview.json)，截图：[连接页](S0/main-72d4353-desktop-connect.png)、[协议示例会话](S0/main-72d4353-desktop-fixtures.png)。开发 worktree mock 的 [bootstrap 预检](S0/unreleased-mock-api.json)通过，但不是 main 联调。

## 快速查看效果

在包含最新 `main` 的工作区执行：

```sh
cd /Users/gongshaojie/Project/mac-bot
./scripts/dev/deploy.sh
./scripts/dev/status.sh
```

`deploy.sh` 会按当前代码可用性编译并部署 `macbotd`、桌面 `.app` 和 Android APK；缺少某条开发线产物时会跳过并打印提示。正式服务使用端口 `7788`，数据目录为 `~/MacBot`，访问密码只从本机文件 `~/.macbot-dev-password` 读取或由部署流程设置，密码内容不写入仓库。

当前可打开桌面客户端，点击“查看协议示例”查看主 Bot 会话；正式 `7788` 和 main Android APK 尚未就绪。`docs/progress/S0/` 已归档桌面连接页与示例会话截图；两端连接正式发布 mock 的证据待补。窗口截图命令写出完整 PNG 后超时，本次截图人工检查并恢复归档，未将退出码记为通过。

桌面端：打开 `~/Applications/MacBot.app`，连接地址填写 `127.0.0.1:7788`，密码填写 `~/.macbot-dev-password` 中的值。

Android 模拟器：需要时先执行：

```sh
./scripts/dev/deploy.sh
```

部署脚本会启动 AVD `macbot_api36`、等待 `sys.boot_completed=1` 并安装 APK。打开 App 后，连接地址填写 `10.0.2.2:7788`；这是模拟器访问 Mac 本机的地址。以后换成同一局域网中的真机时，填写 `192.168.31.162:7788`。密码与桌面端相同。

查看服务状态、进程、端口和最近日志：

```sh
./scripts/dev/status.sh
```

需要验证 mock 或 S0 骨架时，启动开发 mock（端口 `7789`）：

```sh
./scripts/dev/mock.sh
python3 scripts/e2e/s0/bootstrap.py
```

mock 的客户端地址是桌面端 `127.0.0.1:7789`、模拟器 `10.0.2.2:7789`，密码为 `dev`。mock 只用于联调，不替代正式服务的 `7788`。

阶段验收时，先打开桌面客户端和模拟器中的 App，再使用集成截图脚本保存画面：

```sh
./scripts/dev/capture.sh S0
```

截图会放在 `docs/progress/S0/`；后续阶段将 `S0` 替换为对应阶段名。桌面截图由 `screencapture` 生成，Android 截图由 `adb exec-out screencap -p` 生成。

## 当前开发线的运行入口

client-mac 已提供实际打包入口，生成带 fixture 资源的 App：

```sh
cd clients/mac
packaging/package.sh debug app
open dist/MacBot.app
```

运行时可用 `packaging/run.sh debug`，或设置 `MACBOT_HOST=127.0.0.1:7788`、`MACBOT_PASSWORD` 后启动正式服务地址。当前 `server/README.md` 仍缺少 `macbotd` 的编译、mock、LaunchAgent 和正式运行命令；`clients/mobile/README.md` 仍缺少 Gradle wrapper、APK 编译、`adb install` 和启动 Activity 命令。部署脚本会逐项探测并跳过尚未发布的组件；集成线会在 `COORDINATION.md` 记录归属和复现信息。协议 `/api/v1/rpc` 的脚本调用应遵循 `docs/PROTOCOL.md`：正式服务请求带 `Authorization: Bearer <密码>`，S0 bootstrap 请求用于确认会话列表可返回。

## 真实模型

真实服务端接入模型时，通过 `/api/v1/rpc` 的 `provider.create` 配置 provider。真实 provider 当前暂不配置，API Key 只能存放在本机环境变量或钥匙串中，不要写入 README、脚本、日志、截图或 Git；在真实 provider 配置完成前，端到端场景统一使用 mock。

## 持续部署与模型配置

集成线可安装本机定时检查，每 60 秒获取并检查 `main` 与 `origin/main` 的提交：

```sh
python3 scripts/dev/watch.py --install
```

源码从 `main` 分支的固定提交导出到 `~/Library/Caches/MacBot/integrator/source/`，不会切换其他开发线的 worktree。远端 `origin/main` 若领先则部署其固定 SHA；本地 main 若领先则部署本地 SHA。二者分叉时停止部署并报告，避免自动选错版本；网络失败时使用已获取的 main 引用。部署与初步 S0 API 检查日志保存在 `~/Library/Caches/MacBot/integrator/watch/`，API 成功不代表两端界面已验收。停止持续检查：`python3 scripts/dev/watch.py --uninstall`。

编译 target 使用独立共享缓存：`~/Library/Caches/MacBot/integrator/target/server` 和 `~/Library/Caches/MacBot/integrator/target/desktop`。每个 main 快照下的 `server/target`、`clients/mac/target` 会链接到对应缓存，减少重复编译；缓存属于本机开发数据，不提交 Git。部署锁会串行化使用同一缓存的构建。

MiniMax CN 国内官方 [Anthropic 接口](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) 的 base URL 为 `https://api.minimax.cn/anthropic`。本机已实测 `MiniMax-M2.5` 文本请求成功，密钥存于登录钥匙串（service `bot.mac.integrator.minimax-cn`，account `macbot-integrator`）；部署服务尚未配置真实 provider，等待执行链路 ready。

真实服务端 ready 后，运行：

```sh
python3 scripts/dev/provider.py --set-defaults
```

该命令通过 `provider.create/update` 写入密钥、`provider.test` 检查接入、`model.upsert` 注册模型，再设置主 Bot 和普通 Bot 默认模型。持续部署检查仅在集成负责人确认非 mock 执行链路 ready、创建本机 `~/Library/Caches/MacBot/integrator/watch/provider-enabled` 标记后执行一次；当前标记未启用。现有 Bot 如果显式指定了其他模型，仍需在客户端设置中改为默认模型。M2.5 不支持图片输入，配置中的 `vision` 为 false。
