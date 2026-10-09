# Mac Bot 集成进度与使用说明

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 现在的状态

截至 `b1ba8f9` 的业务状态，仓库仍处于 S0 契约与工程骨架阶段。协议和完整场景文档已经提交，但服务端、桌面端、Android 端尚未完成三线合入，因此还没有通过任何阶段的联调验收。

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 阻塞：main 尚无可运行组件 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未验收 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未验收 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未验收 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未验收 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。最新结果和遗留问题以根目录 `COORDINATION.md` 为准。当前证据见 [S0 verification.json](S0/verification.json)：部署已实测逐项跳过，bootstrap 因 mock 未启动失败；没有产品截图。

## 快速查看效果

在包含最新 `main` 的工作区执行：

```sh
cd /Users/gongshaojie/Project/mac-bot
./scripts/dev/deploy.sh
./scripts/dev/status.sh
```

`deploy.sh` 会按当前代码可用性编译并部署 `macbotd`、桌面 `.app` 和 Android APK；缺少某条开发线产物时会跳过并打印提示。正式服务使用端口 `7788`，数据目录为 `~/MacBot`，访问密码只从本机文件 `~/.macbot-dev-password` 读取或由部署流程设置，密码内容不写入仓库。

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

三条开发线的 README 已说明产物归属，但当前 `main` 尚未提供可执行的编译、打包、安装命令：

- `server/README.md`：缺少 `macbotd` 的 `cargo build`、mock 启动、LaunchAgent 安装和正式运行命令。
- `clients/mac/README.md`：缺少桌面 `.app` 的编译、打包、安装和启动命令。
- `clients/mobile/README.md`：缺少 Gradle wrapper、APK 编译、`adb install` 和启动 Activity 命令。

在这些命令补齐前，部署脚本应逐项探测并跳过缺失产物；集成线会在 `COORDINATION.md` 记录归属和复现信息。协议 `/api/v1/rpc` 的脚本调用应遵循 `docs/PROTOCOL.md`：正式服务请求带 `Authorization: Bearer <密码>`，S0 bootstrap 请求用于确认会话列表可返回。

## 真实模型

真实服务端接入模型时，通过 `/api/v1/rpc` 的 `provider.create` 配置 provider。API Key 只能存放在本机环境变量或钥匙串中，不要写入 README、脚本、日志、截图或 Git；在真实 provider 配置完成前，端到端场景统一使用 mock。

## 持续部署与模型配置

集成线可安装本机定时检查，每 60 秒检查本地 `main` 的提交：

```sh
python3 scripts/dev/watch.py --install
```

源码从本地 `main` 的固定提交导出到 `~/Library/Caches/MacBot/integrator/source/`，不会切换其他开发线的 worktree。远端更新需要先合入本地 `main`；fetch 不等于合入。部署与初步 S0 API 检查日志保存在 `~/Library/Caches/MacBot/integrator/watch/`，API 成功不代表两端界面已验收。停止持续检查：`python3 scripts/dev/watch.py --uninstall`。

MiniMax CN 国内官方 [Anthropic 接口](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) 的 base URL 为 `https://api.minimax.cn/anthropic`；本机已实测 `MiniMax-M2.5` 文本调用成功。用户密钥保存在登录钥匙串，service 为 `bot.mac.integrator.minimax-cn`，account 为 `macbot-integrator`；服务端未就绪前不会标记为已配置。

真实服务端 ready 后，运行：

```sh
python3 scripts/dev/provider.py --set-defaults
```

该命令通过 `provider.create/update` 写入密钥、`provider.test` 检查接入、`model.upsert` 注册模型，再设置主 Bot 和普通 Bot 默认模型。持续部署检查会在服务首次可用时执行一次；现有 Bot 如果显式指定了其他模型，仍需在客户端设置中改为默认模型。M2.5 不支持图片输入，配置中的 `vision` 为 false。
