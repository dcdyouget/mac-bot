# Mac Bot 集成进度与使用说明

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 现在的状态

桌面安装版 `eb088fa`、mock `0ed0a07` 已在 Mac mini 运行；Android 当前由 client-android 安装签名 Release 做专项验证（S0 时验收的安装版为 `08462a4`）。两端已连接 mock 并实际看到主 Bot/群/Bot 会话列表；桌面开发期使用本机文件保存 Host 密码，本次无钥匙串提示。三条开发线 S0 已打卡，S0 联调通过；正式 `7788` 和真实模型执行尚未 ready，当前只运行 `7789/dev` mock。

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 通过：两端连接 mock 并看到会话列表 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未验收 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未验收 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未验收 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未验收 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。

桌面功能 checkpoint `2633ff9` 的干净快照编译通过，但打包脚本依赖 `.git` 而失败，尚未替换安装版；client-mac 已报告 archive 修复 `58a6081`，等待最终 main SHA。集成已将固定 SHA 传入打包接口。mock `d173bab` 的 release binary 已构建暂存，等待共享 QA 窗口交回后升级。详情见 [构建暂存记录](S1/checkpoint-2633ff9-build.json)；这些准备不代表 S1–S5 联调通过。最新结果和遗留问题以根目录 `COORDINATION.md` 为准。契约发布核查见 [contracts-published.json](S0/contracts-published.json)。历史初次预检见 [verification.json](S0/verification.json)。最新桌面部署核查见 [main-desktop-preview.json](S0/main-desktop-preview.json)，截图：[连接页](S0/main-72d4353-desktop-connect.png)、[协议示例会话](S0/main-72d4353-desktop-fixtures.png)。开发 worktree mock 的 [bootstrap 预检](S0/unreleased-mock-api.json)通过，但不是 main 联调。

## 快速查看效果

在包含最新 `main` 的工作区执行：

```sh
cd /Users/gongshaojie/Project/mac-bot
./scripts/dev/deploy.sh
./scripts/dev/status.sh
```

`deploy.sh` 会按当前代码可用性编译并部署 `macbotd`、桌面 `.app` 和 Android APK；缺少某条开发线产物时会跳过并打印提示。正式服务使用端口 `7788`，数据目录为 `~/MacBot`，访问密码只从本机文件 `~/.macbot-dev-password` 读取或由部署流程设置，密码内容不写入仓库。

当前两端的 Mock Host 均可查看会话。mock 最新 [0ed0a07 API 预检](S0/mock-0ed0a07-precheck.json) 通过；接管实时 driver/tab 修复由两端继续验证，不计作 S4 通过。更新后的 [桌面画面](S0/mock-0ed0a07-installed-eb088fa-desktop.png) 与 [Android 当前画面](S0/mock-0ed0a07-android-release-current.png) 已归档。mock 重启后桌面当前聊天暂为空，但 RPC 历史有4条；client-mac 已确认 bootstrap 重置后缺少历史重拉，正在修复，S1历史恢复未通过。最新 [S0 验证记录](S0/main-s0-current.json)、[桌面会话截图](S0/main-eb088fa-desktop-sessions.png) 和 [Android 会话截图](S0/main-08462a4-android.png) 已归档。历史 [首次不通过记录](S0/main-s0-integration.json) 保留；桌面 Keychain 阻塞和刷新问题已修复。窗口截图命令写出完整 PNG 后超时，已人工检查并恢复归档，未将命令退出码记为通过。

桌面端：双击打开 `~/Applications/MacBot.app`；当前 mock 地址为 `127.0.0.1:7789`，密码 `dev`，开发部署已启用文件凭据后端。正式服务 ready 后填写 `127.0.0.1:7788`，密码取自 `~/.macbot-dev-password`。

Android 模拟器：需要时先执行：

```sh
./scripts/dev/deploy.sh
```

部署脚本会启动 AVD `macbot_api36`、等待 `sys.boot_completed=1` 并安装 APK。打开 App 后，当前 mock 地址填写 `10.0.2.2:7789`，密码 `dev`；正式服务 ready 后填写 `10.0.2.2:7788`。这是模拟器访问 Mac 本机的地址。以后换成同一局域网中的真机时，填写 `192.168.31.162:7788`。密码与桌面端相同。

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

服务端和 Android 编译运行命令已发布：

```sh
cargo build --release --manifest-path server/Cargo.toml -p macbotd
cd clients/mobile
./gradlew :androidApp:assembleDebug
```

LaunchAgent、独立 MACBOT_HOME、APK 安装与 Activity 命令见 `server/README.md`、`clients/mobile/README.md`。Android 编译使用 JDK 21 与 API 37.0，模拟器和 target 仍为 API 36；`deploy.sh` 会自动探测产物。当前 mock-only 发布期使用 `MACBOT_SKIP_PRODUCTION=1 ./scripts/dev/deploy.sh` 和 `./scripts/dev/mock.sh`；正式访问密码仍只存于 `~/.macbot-dev-password`。


## 真实模型

真实服务端接入模型时，通过 `/api/v1/rpc` 的 `provider.create` 配置 provider。按用户最新授权，开发期使用本机明文凭据文件，放在仓库外、权限 `0600`；不写入 README、脚本、日志、截图或 Git。MiniMax Key 位于 `~/MacBot-dev-secrets/minimax-cn.key`，配置脚本只读取环境变量或此文件，不再访问钥匙串。真实 provider 当前暂不配置，端到端场景继续使用 mock。

## 持续部署与模型配置

集成线可安装本机定时检查，每 60 秒获取并检查 `main` 与 `origin/main` 的提交：

```sh
python3 scripts/dev/watch.py --install
```

Android 当前是归属线安装的签名 Release；Debug 签名不同，专项验证期间保留安装/重启 hold，不尝试替换或卸载。当前本机 `watch/mock-only` 标记暂缓正式服务部署；`7788` 执行 ready 后由集成负责人移除此标记。共享模拟器进行专项 UI 检查时，本机 `watch/ui-validation-hold` 标记暂缓周期部署，交回后移除恢复；只部署桌面可显式指定 `MACBOT_SKIP_PRODUCTION=1 MACBOT_SKIP_ANDROID=1`。

源码从 `main` 分支的固定提交导出到 `~/Library/Caches/MacBot/integrator/source/`，不会切换其他开发线的 worktree。远端 `origin/main` 若领先则部署其固定 SHA；本地 main 若领先则部署本地 SHA。二者分叉时停止部署并报告，避免自动选错版本；网络失败时使用已获取的 main 引用。部署与初步 S0 API 检查日志保存在 `~/Library/Caches/MacBot/integrator/watch/`，API 成功不代表两端界面已验收。停止持续检查：`python3 scripts/dev/watch.py --uninstall`。

编译 target 使用独立共享缓存：`~/Library/Caches/MacBot/integrator/target/server` 和 `~/Library/Caches/MacBot/integrator/target/desktop`。每个 main 快照下的 `server/target`、`clients/mac/target` 会链接到对应缓存，减少重复编译；缓存属于本机开发数据，不提交 Git。部署锁会串行化使用同一缓存的构建。

MiniMax CN 国内官方 [Anthropic 接口](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) 的 base URL 为 `https://api.minimax.cn/anthropic`。本机已实测 `MiniMax-M2.5` 文本请求成功，开发凭据现存于 `~/MacBot-dev-secrets/minimax-cn.key`。可用 `MINIMAX_API_KEY_FILE` 指向新的本机文件，或设置 `MINIMAX_API_KEY`；部署服务尚未配置真实 provider，等待执行链路 ready。

本机 `watch/file-secrets` 标记让部署使用 `MACBOT_SECRET_BACKEND=file`。桌面 Host 密码保存于 `~/Library/Application Support/MacBot/development-secrets.json`（文件0600、目录0700），此模式不访问钥匙串。服务端文件后端仍等待 adapter 完整发布；用户后续可替换本机 MiniMax Key 文件，再重新运行 provider 配置命令。

真实服务端 ready 后，运行：

```sh
python3 scripts/dev/provider.py --set-defaults
```

该命令通过 `provider.create/update` 写入密钥、`provider.test` 检查接入、`model.upsert` 注册模型，再设置主 Bot 和普通 Bot 默认模型。持续部署检查仅在集成负责人确认非 mock 执行链路 ready、创建本机 `~/Library/Caches/MacBot/integrator/watch/provider-enabled` 标记后执行一次；当前标记未启用。现有 Bot 如果显式指定了其他模型，仍需在客户端设置中改为默认模型。M2.5 不支持图片输入，配置中的 `vision` 为 false。
