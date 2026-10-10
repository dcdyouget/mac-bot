> 修复包 `3d6b289c3af61111e0004d8bf300330461528ef7`（SHA-256 `120af2fa8a2890da3806679b052f8d7adb1f1697e02e6c69ee7b0efff99af00a`）已成功 native 安装；PackageInfo relocation 为空，完整 smoke 通过。管理页密码已由用户设置，file backend、MiniMax CN/MiniMax-M2.5 配置已完成；两端已安装并连接。之后 cf6b5bb 仅作为已核验的用户服务 App 载荷更新运行，不代表重新安装 cf6b5bb native pkg，系统 Installer receipt 仍为 3d6b289。cf6b5bb PID84786 的只读复核确认 seq15 orphan 唯一 run 恢复、memory_search 返回 1 entry、seq16 回复成功。桌面当前为已部署的 bdc17cdf 诊断版；AX 可读且截图可取，routine 详情已实际查看，但坐标点击返回 `noWindowsAvailable`，不能标为锁屏，持续画面仍未闭环。Android 9f9a1cf 已同签名覆盖安装并进入 B 待验收；B 两端 review 画面出现 request timeout（Android 30 秒），不能记连接稳定通过；此前 953c53b 真实输入提交为 green，但 native 交还仍未闭环。S1–S4 fresh 场景仍未收口，S5 不通过。旧3635008包不再重试。

> 首次真实安装失败：macOS将payload重定位到备份App，postinstall找不到固定源路径。原业务数据164job哈希保持，但备份App已被写过，不再作为原样恢复来源；空的新home为root所有。修复包冻结之前不得重试旧包。见 [fresh-installer-relocation-failure.json](fresh-installer-relocation-failure.json)。

# S5 全新安装演练计划

状态：**执行中，修复包已安装并完成密码、模型配置，不能计 S5 通过**。既有 S0–S4 阶段结论保持；server `cf6b5bbc40def3a6015a41998c94905cbb35965f` 已作为 user-App payload 部署，PID `84786`，seq15 orphan 已恢复为唯一 run，memory_search 返回 1 entry，seq16 回复成功。系统 native receipt 仍为 3d6b289。桌面 bdc17cdf AX 可读且截图可取，routine 详情已实际查看，但坐标点击返回 `noWindowsAvailable`，不是锁屏证据，持续画面仍在调查；Android 9f9a1cf 已同签名覆盖安装并进入 B 待验收，B 两端 review 画面出现 request timeout（Android 30 秒），不能记连接稳定通过；此前 953c53b 真实输入提交为 green，但 native 交还仍未闭环。Fresh S1 工具闭环和 kill-9 恢复已有证据；fresh S2 A 仍待验收未 confirm_done，B 已在同 run 以修正 slug 成功发起 request_review，旧报告退出项 PARTIAL 保留；fresh S3 桌面与 Android 跨 run memory 截图已实际查看；Android 画面同时重连并出现 30 秒超时，不计整 S3 通过；fresh S4 schedule 晚 94.255847 秒，目标通知被旧 A 洪水挤出且未验收打开，不能记通过。旧现场已离线移动至既定仓库外备份，164个任务哈希核对保持；mock与Android数据保留。首次自动重定位失败及被污染的 backup App 仍保留为历史边界；当前候选、安装和后续进度见本计划、独立 audit JSON 及 [fresh-evidence-index.md](fresh-evidence-index.md)。

## 前置条件与路径

- server：当前 native receipt/服务仍为已安装的 `3d6b289c3af61111e0004d8bf300330461528ef7`；`cf6b5bbc40def3a6015a41998c94905cbb35965f` 已部署为 user-App payload，PID `84786`，seq15 orphan 唯一 run、memory_search 1 entry 和 seq16 回复已复核，见 [fresh-server-cf6b5bb-update.json](fresh-server-cf6b5bb-update.json)；系统 receipt 仍为 3d6b289。管理页密码、file backend、模型配置和现有两端连接事实保持；设置密码前的 `setup_required` 观测缺口见 [fresh-setup-observation-audit.json](fresh-setup-observation-audit.json)。
- desktop：当前已部署 bdc17cdf 诊断版，见 [desktop-dmg-bdc17cdf-candidate.json](desktop-dmg-bdc17cdf-candidate.json) 和 [fresh-desktop-bdc17cdf-update.json](fresh-desktop-bdc17cdf-update.json)。首帧已 applied 但未触发重绘，仍在调查，不能据此计 S4 或 S5 通过；此前 0e866f5、ad2d915、3b017b1 和 508f325 仅作为历史 fresh 安装记录保留。
- Android：已同签名覆盖安装并保留应用数据的 `9f9a1cf9dfab53a9fdf7f3bf4514b59e0c097120` Release，当前进入 B 待验收；B 两端 review 画面出现 request timeout（Android 30 秒），不能记连接稳定通过；此前 953c53b 真实输入提交为 green，但 native 交还仍未闭环。此前连续画面、控制延迟和 release pending 证据见 [fresh-s4-android-screen-takeover.json](fresh-s4-android-screen-takeover.json)、[fresh-s4-android-control-delay-audit.json](fresh-s4-android-control-delay-audit.json)、[fresh-s4-android-release-pending.png](fresh-s4-android-release-pending.png)。性能、native release 和双端联合验收仍 pending。
- sidecar：agent-browser 0.39.0。pkg为未签名开发候选，桌面为ad hoc签名；不冒充Developer ID签名公证分发。
- Installer 管理员认证和管理页新密码的输入、确认、提交已由用户接手完成；密码值不写入日志、截图或提交。设置密码前的 `setup_required` 观测缺口保留在 [fresh-setup-observation-audit.json](fresh-setup-observation-audit.json)，不因此重装或伪造结果。

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
3. 对正式 LaunchAgent 做 bootout，确认 7788 已关闭后，复用现有 [fresh-install-journal.json](fresh-install-journal.json) 中的仓库外 `0700` backup_root；本次离线移动已完成，业务数据和 164 个 job hash 保持。不要另建新时间戳目录，也不要先 copy live `~/MacBot` 再在后面 move 同一目录。

```sh
umask 077
BACKUP="$HOME/Library/Application Support/MacBot/S5-backup-20261010-gcdb9tuk"
test -d "$BACKUP"
# 复用现有空 backup；先人工确认下列分类目录没有业务文件，再移动 live 路径。
find "$BACKUP" -type f -o -type l
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

核对 `/Applications/MacBot Server.app`、`~/Applications/MacBot Server.app`、LaunchAgent、source stamp、daemon/sidecar SHA，并确认 LaunchAgent PID 与 7788 listener PID 一致。规范要求全新 `~/MacBot` 先返回 `setup_required=true`；本次安装前没有捕获该响应，不能把安装后的 `setup_required=false` 解释为安装前结果，也不据此重装；缺口见 [fresh-setup-observation-audit.json](fresh-setup-observation-audit.json)。

通过 `http://127.0.0.1:7788/admin` 设置新密码；本次已由用户本人接手输入、确认和提交。具体密码值不写入本计划、日志、截图或提交。无显示器时 PLAN 的 `macbot passwd` 是独立安装方式，不能用来绕过本次管理页验收。认证 bootstrap 通过后，在新的 secret 目录显式启用开发 file backend，避免默认 Keychain 弹窗；本次 file backend 和模型默认配置已完成，journal 见 `fresh-file-backend.json` 与 `fresh-rpc/`：

- 新建仓库外 `MACBOT_SECRET_DIR`，权限 `0700`，不复制旧 secret。
- 在用户 LaunchAgent 环境中设置 `MACBOT_SECRET_BACKEND=file` 和该目录，再按 server README 重载 LaunchAgent。
- `scripts/dev/provider.py` 可读取用户已授权的现有 `~/MacBot-dev-secrets/minimax-cn.key`（或 `MINIMAX_API_KEY_FILE` 指定文件），通过 provider RPC 将 key 写入新的 secret 目录；原 key 文件不复制，值不出现在日志、JSON、截图或 Git。

### C. 安装两端

先正常退出旧桌面 App，将 `~/Applications/MacBot.app` 和已有 `/Applications/MacBot.app` 分目录保留到仓库外备份，避免同时运行同 bundle 的两版。桌面正式 DMG 已安装并连接；后续继续保留截图和场景证据。桌面使用正式 DMG，不用 `deploy.sh` debug App：

```sh
hdiutil attach -nobrowse -readonly "/path/to/MacBot-<ready-sha>.dmg"
# 将挂载卷内已 codesign 校验的 MacBot.app 拖入/复制到 /Applications
codesign --verify --deep --strict "/Applications/MacBot.app"
hdiutil detach "/Volumes/<Mac Bot 卷名>"
open "/Applications/MacBot.app"
```

桌面已填入 `127.0.0.1:7788` 和新密码并完成连接观察。Android 只使用已核对的同签名 Release：

```sh
adb -s emulator-5554 install -r "/path/to/androidApp-release.apk"
adb -s emulator-5554 shell pm path bot.mac.mobile
```

模拟器填 `10.0.2.2:7788`；真机以后填 `192.168.31.162:7788`。不执行 `adb uninstall`、`pm clear` 或重置 AVD，保留客户端数据和签名更新能力。

### D. 完整场景与恢复

1. 运行现有 `scripts/e2e/s1/`，再按 PLAN/DESIGN 顺序运行 S2、S3、S4；fresh S1 工具闭环和 kill-9 恢复已有证据，桌面 cache 已收到 50 段全文且 after-layout 图已实际查看，Android 后续真实输入提交为 green，但 native 交还与性能仍待补齐，边界归档在 [fresh-evidence-index.md](fresh-evidence-index.md)。fresh S2 已完成两个群并行和运行中同 assignment/run 插话；A 仍待验收未 confirm_done，B 两端 review 截图已查看但出现 request timeout，已在同 run 以修正 slug 成功发起 request_review，旧报告退出项 PARTIAL 保留。fresh S3 seq15 orphan 唯一 run、memory_search 1 entry、seq16 回复已复核，桌面与 Android 跨 run memory 截图已实际查看；Android 画面同时重连并出现 30 秒超时，不计整 S3 通过。fresh S4 Android native 交还未闭环；桌面 routine 详情已实际查看而持续画面仍缺；routine schedule 晚 94.255847 秒，目标通知被旧 A 洪水挤出且打开未验收，routine 已禁用。旧持久 pending 仅保留在离线备份，不是当前 fresh 阻塞，也不在 fresh 中续办旧群。
2. 每阶段保存桌面 `screencapture`、Android `adb exec-out screencap -p`、RPC/e2e JSON、source/PID 和实际断言；健康、API、mock 或单张 owner 图不能替代双端联合验收。
3. S5 通过条件是 pkg 安装、管理页设密码、两端连接、完整场景和截图全部完成。失败时保留新现场和证据；停止新 LaunchAgent 后，从仓库外备份恢复 `~/MacBot`、plist、对应 App 路径和 CLI links，恢复原外部 file secret 目录，Keychain 不变，再核对 source/PID/health。不要清空 Android 数据或密钥，也不要混用新旧数据。

当前状态：S5 已完成离线备份、3d6b289 native pkg 安装、管理页密码、file backend/模型配置以及两端安装连接；cf6b5bb user-App payload 已部署 PID84786，seq15 orphan 唯一 run、memory_search 1 entry、seq16 回复已复核；系统 receipt 仍为 3d6b289。桌面 bdc17cdf AX 可读，routine 详情已实际查看但持续画面仍缺；Android 9f9a1cf 已同签名覆盖安装并进入 B 待验收，B review 画面出现 request timeout，真实输入提交 green 但 native 交还未闭环。S1 联合实时流式与性能仍待完成；S2 A 未 confirm_done，B request_review 已成功但旧报告退出项 PARTIAL；S4 schedule 晚 94.255847 秒，目标通知被旧 A 洪水挤出且打开未验收，X、桌面持续画面和联合验收待完成。设置密码前 `setup_required` 未观测，保留为证据缺口；本计划不构成验收通过。
