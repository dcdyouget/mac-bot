# Mac Bot 集成进度与使用说明

本文面向在 Mac mini（Apple M4，局域网地址 `192.168.31.162`）上查看 Mac Bot 效果的人。服务端运行在这台 Mac 上，Android 客户端运行在本机的 `macbot_api36` 模拟器里。v1 不包含 iOS。

## 现在的状态

桌面安装版 `fb0da7c`、mock `f368824` 已在 Mac mini 运行；Android 当前由 client-android 安装签名 Release 做专项验证（S0 时验收的安装版为 `08462a4`）。两端已连接 mock 并实际看到主 Bot/群/Bot 会话列表；桌面开发期使用本机文件保存 Host 密码，本次无钥匙串提示。三条开发线 S0 已打卡，S0 联调通过；正式 `7788` 和真实模型执行尚未 ready，当前只运行 `7789/dev` mock。

| 阶段 | 目标 | 联调状态 | 截图目录 |
|---|---|---|---|
| S0 | 桌面端和 Android 模拟器连接 mock 并看到会话列表 | 通过：两端连接 mock 并看到会话列表 | `docs/progress/S0/` |
| S1 | 真实服务端上的单 Bot 对话、工具和轨迹恢复 | 未验收 | `docs/progress/S1/` |
| S2 | 主 Bot、群协作、插话和待验收 | 未验收 | `docs/progress/S2/` |
| S3 | 技能、仪表盘、搜索和记忆 | 未验收 | `docs/progress/S3/` |
| S4 | 浏览器画面、接管、定时任务和通知 | 未验收 | `docs/progress/S4/` |
| S5 | pkg 全新安装、两端连接和完整场景 | 未验收 | `docs/progress/S5/` |

每个阶段只有在三条开发线都在 `COORDINATION.md` 打卡，并且集成线完成真实联调、保存两端截图后，才会标记为“通过”。

桌面修复版 `fb0da7c` 已从干净 archive 打包并安装，启动后总管4条历史消息实见：[安装核查](S1/main-fb0da7c-installed-precheck.json)、[窗口截图](S1/main-fb0da7c-installed-history.png)。CUA 绑定返回 `cgWindowNotFound`，技能页点击及 mock 重连后的历史恢复仍待复验；截图 finalize 超时后已人工核图恢复。已解除 `2633ff9` 无 `.git` 打包阻断。mock `f368824` 的[成员/pending预检](S2/mock-f368824-members-api.json)通过，登录群/项目/公告均4成员；搜索[预检](S3/mock-search-0e882ac-api.json)通过。Workbench 扁平协议修复已发布 `8791fa1`，共享 mock 尚未消费，等 Android 最终验证交回。构建、静态截图和 mock API 结果不代表 S1–S5 联调通过。最新结果和遗留问题以根目录 `COORDINATION.md` 为准。

## 快速查看效果

在包含最新 `main` 的工作区执行：

```sh
cd /Users/gongshaojie/Project/mac-bot
./scripts/dev/deploy.sh
./scripts/dev/status.sh
```

`deploy.sh` 会按当前代码可用性编译并部署 `macbotd`、桌面 `.app` 和 Android APK；缺少某条开发线产物时会跳过并打印提示。正式服务使用端口 `7788`，数据目录为 `~/MacBot`，访问密码只从本机文件 `~/.macbot-dev-password` 读取或由部署流程设置，密码内容不写入仓库。

S0 两端会话列表验收已归档：[记录](S0/main-s0-current.json)、[桌面会话](S0/main-eb088fa-desktop-sessions.png)、[Android 会话](S0/main-08462a4-android.png)。当前安装 `fb0da7c` 已实际显示总管历史，技能页及重连回归未完成；Android 签名 Release 由归属线冻结最终验证，保留 AVD/安装 hold。历史失败证据保留。

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

Android 部署用 `MACBOT_ANDROID_VARIANT=debug|release` 选择构建，未指定时沿用本机 `android-installed-variant` 记录，否则默认 Debug；本机当前记录为 Release。Release 从 `~/.local/share/macbot/android-signing/release.env` 加载签名环境，可用 `MACBOT_ANDROID_SIGNING_ENV` 指定其他仓库外文件。仅安装 `androidApp-release.apk`；缺少签名或安装签名不匹配时失败并保留已有 App/数据，不卸载、不安装 unsigned APK。变体逻辑已用临时假 Gradle/假签名验证，实际 Release 构建与更新待 Android 交回后执行。

源码从 `main` 分支的固定提交导出到 `~/Library/Caches/MacBot/integrator/source/`，不会切换其他开发线的 worktree。远端 `origin/main` 若领先则部署其固定 SHA；本地 main 若领先则部署本地 SHA。二者分叉时停止部署并报告，避免自动选错版本；网络失败时使用已获取的 main 引用。部署与初步 S0 API 检查日志保存在 `~/Library/Caches/MacBot/integrator/watch/`，API 成功不代表两端界面已验收。停止持续检查：`python3 scripts/dev/watch.py --uninstall`。

编译 target 使用独立共享缓存：`~/Library/Caches/MacBot/integrator/target/server` 和 `~/Library/Caches/MacBot/integrator/target/desktop`。每个 main 快照下的 `server/target`、`clients/mac/target` 会链接到对应缓存，减少重复编译；缓存属于本机开发数据，不提交 Git。部署锁会串行化使用同一缓存的构建。

MiniMax CN 国内官方 [Anthropic 接口](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) 的 base URL 为 `https://api.minimax.cn/anthropic`。本机已实测 `MiniMax-M2.5` 文本请求成功，开发凭据现存于 `~/MacBot-dev-secrets/minimax-cn.key`。可用 `MINIMAX_API_KEY_FILE` 指向新的本机文件，或设置 `MINIMAX_API_KEY`；部署服务尚未配置真实 provider，等待执行链路 ready。

本机 `watch/file-secrets` 标记让部署使用 `MACBOT_SECRET_BACKEND=file`。桌面 Host 密码保存于 `~/Library/Application Support/MacBot/development-secrets.json`（文件0600、目录0700），此模式不访问钥匙串。服务端文件后端仍等待 adapter 完整发布；用户后续可替换本机 MiniMax Key 文件，再重新运行 provider 配置命令。

真实服务端 ready 后，运行：

```sh
python3 scripts/dev/provider.py --set-defaults
```

该命令通过 `provider.create/update` 写入密钥、`provider.test` 检查接入、`model.upsert` 注册模型，再设置主 Bot 和普通 Bot 默认模型。持续部署检查仅在集成负责人确认非 mock 执行链路 ready、创建本机 `~/Library/Caches/MacBot/integrator/watch/provider-enabled` 标记后执行一次；当前标记未启用。现有 Bot 如果显式指定了其他模型，仍需在客户端设置中改为默认模型。M2.5 不支持图片输入，配置中的 `vision` 为 false。
