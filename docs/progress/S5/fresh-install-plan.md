# S5 全新安装演练计划

状态：**未执行，不能计 S5 通过**。这是可逆操作计划，不是验收记录。

最新候选（2026-10-10）：S0–S3已通过；S4 的 X/Android 接管核心链路已有真实证据，但交还后原聊天 block 仍 pending、Android 接管按钮残留，UX 尚未闭环。server 候选为 `9c9cd7d5e4d154b24ba0aed793325158fe67ff0a`，桌面候选为 `ba0e8f17b5dfd779efc5cd4b41c7168a57d70b72`；pkg 及审计见 [server-pkg-9c9cd7d-verified.json](server-pkg-9c9cd7d-verified.json)，包SHA `f88b41044e6af99b115e751cbab4284a33a998bf261bd087006a17e093882814`，sidecar 为 agent-browser 0.39.0。Android `04b0c3d515cabbc55681f1c630b115674c8edb94` Release APK 已同签名 `install -r` 保留数据并完成长聊天最新消息/上滑实看，但仍仅作后续 S5 候选（见 [android-04b0c3d-release-candidate.json](android-04b0c3d-release-candidate.json)）；未执行 fresh。用户属于admin但sudo不能免密，安装时需要用户完成管理员认证，管理页新密码按下文交接。历史版本说明不覆盖本段。

当前停止条件：server 以 `9c9cd7d5e4d154b24ba0aed793325158fe67ff0a` pkg 作为待安装候选，桌面候选为 `ba0e8f17b5dfd779efc5cd4b41c7168a57d70b72`；Android `04b0c3d` 已完成保留数据的常规 `install -r` 聊天实看，但未计入 S5 fresh 实测。Mac 已解锁。S0、S1、S3 已通过；S4 X/Android 核心链路有证据但交还后的 pending block/UI 残留仍待 server/client 修复与回归；S5 fresh 未执行。原旧 siblings/未知审批保持，不自动取消或批准。

## 前置条件与路径

- server 当前 fresh 候选为 `9c9cd7d5e4d154b24ba0aed793325158fe67ff0a`，`.pkg` SHA-256 为 `f88b41044e6af99b115e751cbab4284a33a998bf261bd087006a17e093882814`，source/build-info/dirty=false、daemon/sidecar SHA 和包 smoke 已核对，见 [server-pkg-9c9cd7d-verified.json](server-pkg-9c9cd7d-verified.json)；这是 unsigned development package，未安装或计 fresh。历史 `bb790b0`、`9c20fc4`、`7481d8e4` 等 pkg 审计记录继续保留。
- client-mac 当前候选为 `ba0e8f17b5dfd779efc5cd4b41c7168a57d70b72`，DMG SHA-256 为 `51e022be7bb6cc974369ec45256ea79e142aeda7ea854307a75adec87c4ae014`，已通过签名校验并作为普通更新安装；不计 fresh，见 [desktop-dmg-ba0e8f1-candidate.json](desktop-dmg-ba0e8f1-candidate.json)。历史 `6eaa91a`、`71fddde` 记录继续保留。
- client-android `04b0c3d` Release APK 为待验收候选，SHA-256 为 `245ff19c9a1e94052d9b01a6f25ac31ff865d73ddb7b4044a97af6cb576098da`，已完成同签名 `adb install -r` 的常规升级实看；正式 fresh 仍未执行，且只允许同签名 `adb install -r`，不卸载、不清空现有数据。
- PLAN 第 6 章 S5 权威顺序是 `pkg → 设置密码 → 两端连接 → 完整场景`（`docs/PLAN.md:524-531`）；服务端安装见 `server/README.md:60-97`，桌面 DMG 见 `clients/mac/README.md:37-52`。

必须区分服务 App 路径：

- 集成开发部署：`~/Applications/MacBotServer.app`（无空格）。
- pkg postinstall：`/Applications/MacBot Server.app`，再复制到 `~/Applications/MacBot Server.app`（有空格）。
- LaunchAgent：`~/Library/LaunchAgents/com.macbot.server.plist`；数据：`~/MacBot`。

`server/macbotd/packaging/uninstall-launchagent.sh` 只删除带空格的用户 App、LaunchAgent 和 CLI 链接，并保留数据；不会清除当前无空格的开发 App。不能只看卸载命令返回值判断清理完成。

## 可逆执行顺序

### A. 收敛、离线备份和隔离

1. 停止 watcher 和会自动部署/重启的后台任务。安全处置正在运行的 job/approval；允许未执行的 durable pending 原样保留，绝不为 fresh 自动 approve 或 cancel。保存当前日志和只读基线：`status.sh`、7788/7789 listener、LaunchAgent PID、executable/source stamp、客户端版本和截图路径。
2. 用户解锁 Mac，并确认 `screen_locked=0`；否则停止，不做原生截图、DMG 打开或双端 UI 验收。
3. 对正式 LaunchAgent 做 bootout，确认 7788 已关闭后，才建立仓库外 `0700` 备份并执行一次离线移动。不要先 copy live `~/MacBot` 再在后面 move 同一目录。

```sh
umask 077
BACKUP="$HOME/Library/Application Support/MacBot/S5-backup-$(date +%Y%m%d-%H%M%S)"
install -d -m 700 "$BACKUP/data" "$BACKUP/user-app" "$BACKUP/system-app" "$BACKUP/launchagent" "$BACKUP/cli"
launchctl bootout "gui/$(id -u)/com.macbot.server" 2>/dev/null || true
lsof -nP -iTCP:7788 -sTCP:LISTEN   # 必须无输出后继续
if test -e "$HOME/MacBot"; then mv "$HOME/MacBot" "$BACKUP/data/MacBot"; fi
if test -e "$HOME/Applications/MacBotServer.app"; then mv "$HOME/Applications/MacBotServer.app" "$BACKUP/user-app/MacBotServer.app"; fi
if test -e "$HOME/Applications/MacBot Server.app"; then mv "$HOME/Applications/MacBot Server.app" "$BACKUP/user-app/MacBot Server.app"; fi
if test -e "/Applications/MacBot Server.app"; then sudo mv \
  "/Applications/MacBot Server.app" "$BACKUP/system-app/MacBot Server.app"; fi
if test -e "$HOME/Library/LaunchAgents/com.macbot.server.plist"; then mv \
  "$HOME/Library/LaunchAgents/com.macbot.server.plist" "$BACKUP/launchagent/com.macbot.server.plist"; fi
for link in "$HOME/.local/bin/macbot" "$HOME/.local/bin/macbotd"; do
  if test -L "$link"; then mv "$link" "$BACKUP/cli/$(basename "$link")"; fi
done
chmod 700 "$BACKUP" # 父目录私有；移动项保留原文件权限，不递归改权限
```

`/Applications/MacBot Server.app` 可能需要管理员权限；按实际权限对该单项使用 `sudo`，pkg 安装本身也需要 root 权限（`sudo installer -pkg ... -target /`）。用户 App、plist、CLI link 分目录保存，避免两个同名 App 在备份中覆盖。原 `~/MacBot` 只离线移动一次，保留其权限和可逆恢复路径。

外部 file backend 的原 `MACBOT_SECRET_DIR` 不复制；原目录和 `minimax-cn.key` 保持原位置、权限和内容。默认 Keychain 项不导出、不删除。`~/.macbot-dev-password` 保持原路径，不读取或打印内容；备份目录路径、权限和存在性可记录。

### B. 安装 pkg 与初始化 fresh home

```sh
sudo installer -pkg "/path/to/MacBot-Server-<ready-sha>.pkg" -target /
```

核对 `/Applications/MacBot Server.app`、`~/Applications/MacBot Server.app`、LaunchAgent、source stamp、daemon/sidecar SHA，并确认 LaunchAgent PID 与 7788 listener PID 一致。全新 `~/MacBot` 应先返回 `setup_required=true`；否则停止并恢复备份，不记 fresh。

通过 `http://127.0.0.1:7788/admin` 设置新密码；使用当前 CUA 工具时，其凭据变更规则要求用户接手新密码输入、确认和提交，集成负责人先准备可操作页面，不代填、不把输入内容写入日志或截图。无显示器时 PLAN 的 `macbot passwd` 是独立安装方式，不能用来绕过本次管理页验收。认证 bootstrap 通过后，在新的 secret 目录显式启用开发 file backend，避免默认 Keychain 弹窗：

- 新建仓库外 `MACBOT_SECRET_DIR`，权限 `0700`，不复制旧 secret。
- 在用户 LaunchAgent 环境中设置 `MACBOT_SECRET_BACKEND=file` 和该目录，再按 server README 重载 LaunchAgent。
- `scripts/dev/provider.py` 可读取用户已授权的现有 `~/MacBot-dev-secrets/minimax-cn.key`（或 `MINIMAX_API_KEY_FILE` 指定文件），通过 provider RPC 将 key 写入新的 secret 目录；原 key 文件不复制，值不出现在日志、JSON、截图或 Git。

### C. 安装两端

先正常退出旧桌面 App，将 `~/Applications/MacBot.app` 和已有 `/Applications/MacBot.app` 分目录保留到仓库外备份，避免同时运行同 bundle 的两版。桌面使用正式 DMG，不用 `deploy.sh` debug App：

```sh
hdiutil attach -nobrowse -readonly "/path/to/MacBot-<ready-sha>.dmg"
# 将挂载卷内已 codesign 校验的 MacBot.app 拖入/复制到 /Applications
codesign --verify --deep --strict "/Applications/MacBot.app"
hdiutil detach "/Volumes/<Mac Bot 卷名>"
open "/Applications/MacBot.app"
```

桌面填 `127.0.0.1:7788` 和新密码。Android 只使用已核对的同签名 Release：

```sh
adb -s emulator-5554 install -r "/path/to/androidApp-release.apk"
adb -s emulator-5554 shell pm path bot.mac.mobile
```

模拟器填 `10.0.2.2:7788`；真机以后填 `192.168.31.162:7788`。不执行 `adb uninstall`、`pm clear` 或重置 AVD，保留客户端数据和签名更新能力。

### D. 完整场景与恢复

1. 运行现有 `scripts/e2e/s1/`，再按 PLAN/DESIGN 顺序运行 S2、S3、S4；当前 S2 的旧持久 pending 可以继续留在离线备份，但不能自动批准/取消；两个原群纠正续办仍须完成并提供 Tester 执行报告、历史 Main 开场和产物闭环证据。
2. 每阶段保存桌面 `screencapture`、Android `adb exec-out screencap -p`、RPC/e2e JSON、source/PID 和实际断言；健康、API、mock 或单张 owner 图不能替代双端联合验收。
3. S5 通过条件是 pkg 安装、管理页设密码、两端连接、完整场景和截图全部完成。失败时保留新现场和证据；停止新 LaunchAgent 后，从仓库外备份恢复 `~/MacBot`、plist、对应 App 路径和 CLI links，恢复原外部 file secret 目录，Keychain 不变，再核对 source/PID/health。不要清空 Android 数据或密钥，也不要混用新旧数据。

当前状态：S5 fresh install 未执行。当前业务部署不构成 fresh；执行前需冻结最终版本并核对本计划列出的 server pkg，本计划文件本身不构成安装或验收通过。

2026-10-10 历史现场补充：server `14a3b18`、桌面 `e14d0a8` Release、Android `5e2fbcc` 签名 Release。桌面 App/DMG 已从干净归档构建并通过签名与镜像校验，正常更新安装不计 fresh；当时的锁屏和搜索误分类记录保留，前文历史版本和停止条件不代表最新部署。

2026-10-10 当前现场补充：server `9c20fc4`、桌面 `6eaa91a`、Android `5e2fbcc`；Mac 已解锁。S0、S1、S3 已通过；S2 历史分支取消确认待用户答复，S4 Chrome 远程调试当前弹窗待用户答复；S5 fresh 仍未执行。

后续只读核对：Chrome授权弹窗已消失，不能等同于批准连接；X验收仍未通过，测试Bot已恢复原headless模式。server9c20fc4正常重启后8.673秒健康，12pending/154jobs保持，当前PID94543。
