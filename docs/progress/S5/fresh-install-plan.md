> 修复包 `3d6b289c3af61111e0004d8bf300330461528ef7`（SHA-256 `120af2fa8a2890da3806679b052f8d7adb1f1697e02e6c69ee7b0efff99af00a`）已成功 native 安装；PackageInfo relocation 为空，完整 smoke 通过。管理页密码已由用户设置，file backend、MiniMax CN/MiniMax-M2.5 配置已完成；两端已安装并连接。之后 server `30212bc7282b1646ee817accab2b0ba6bd9eb2ea` 仅作为已核验的用户服务 App 载荷更新运行，当前 PID `4843`，health 正常且 `setup_required=false`，jobs 哈希、pending 和 plist 保持不变，不是重新安装 native pkg，系统 Installer receipt 仍为 3d6b289。首次 bootstrap 的 I/O error5、服务退出和 7788 无监听保留在更新审计，确认服务 absent 后再次 bootstrap 成功；47fdd2a 旧 preflight timeout 仍是历史快照。30212bc 只读 bootstrap/workbench/routine 已完成，耗时 9.033/0.144/0.103 秒；另一次 warm bootstrap 为 8.071 秒，与 47fdd2a 的 30/37/14 秒点测相比仅表示本次观测改善，不是全面性能通过。桌面 bdc17cdf 置前后仍显示旧画面，Dock 工具超时，BetterDisplay occlusion 待用户 A/B 对照。Android `01f14d78f203456ca9c64d8483578fd02c66d68a` 已同签名覆盖安装并保留数据；native release 服务端已 done 但 browser false，手动关闭后的截图仍显示 Bot 操作中、无输入、接管按钮和持续 fps，不能标为及时或自动交还 UI 通过。S1–S4 fresh 场景仍未收口，S5 不通过。旧3635008包不再重试.

本轮记录中的安装、密码设置和两端连接步骤已执行；fresh 不应重跑这些有副作用步骤，部署前的 preflight 数值仅是历史快照，不代表当前状态。
> 首次真实安装失败：macOS将payload重定位到备份App，postinstall找不到固定源路径。原业务数据164job哈希保持，但备份App已被写过，不再作为原样恢复来源；空的新home为root所有。修复包冻结之前不得重试旧包。见 [fresh-installer-relocation-failure.json](fresh-installer-relocation-failure.json)。

# S5 全新安装演练计划

状态：**执行中，修复包已安装并完成密码、模型配置，不能计 S5 通过**。既有 S0–S4 阶段结论保持；server `30212bc7282b1646ee817accab2b0ba6bd9eb2ea` 已部署，PID `4843`，health 正常且 `setup_required=false`，首次 bootstrap I/O error5 和 service absent/7788 无监听失败边界保留，随后重试成功；jobs 哈希、pending、plist 均未变化，系统 native receipt 仍为 3d6b289。30212bc 只读 bootstrap/workbench/routine 已完成，耗时分别为 9.033/0.144/0.103 秒；47fdd2a 的旧 preflight timeout 仅作历史快照。provider 429 仍阻断新模型场景，未创建新任务。Android `01f14d78f203456ca9c64d8483578fd02c66d68a` 已安装并保留数据；native release 已 done 但 browser false，手动关闭后的截图仍显示 Bot 操作中、无输入、接管按钮和持续 fps，不能写及时或自动交还 UI 通过。桌面 bdc17cdf 置前后仍为旧画面，Dock 工具 timeout，BetterDisplay occlusion 待用户对照；adapter 性能补丁尚未部署。Fresh S1 工具闭环和 kill-9 恢复已有证据；fresh S2 A 仍待验收未 confirm_done，B 已在同 run 以修正 slug 成功发起 request_review，旧报告退出项 PARTIAL 保留；fresh S3 与 S4 仍为局部证据。安装、后续进度和失败边界见本计划、独立 audit JSON 及 [fresh-evidence-index.md](fresh-evidence-index.md)。

## 前置条件与路径

- server：当前 native receipt 仍为已安装的 `3d6b289c3af61111e0004d8bf300330461528ef7`；`30212bc7282b1646ee817accab2b0ba6bd9eb2ea` 已部署为 user-App payload，PID `4843`，health 正常且 `setup_required=false`，首次 bootstrap I/O error5 和 service absent/7788 无监听失败边界保留，随后重试成功；jobs 哈希、pending、plist 均未变化，见 [fresh-server-30212bc-update.json](fresh-server-30212bc-update.json)。30212bc 只读 bootstrap/workbench/routine 耗时为 `9.033/0.144/0.103s`，另一次 warm bootstrap 为 `8.071s`；与 47fdd2a 的 30/37/14 秒点测相比仅表示本次观测改善，不是全面性能通过，见 [fresh-server-30212bc-readonly.json](fresh-server-30212bc-readonly.json)。47fdd2a 的旧 preflight timeout 与只读记录保留为历史快照，不覆盖或替代当前证据。
- desktop：当前已部署 bdc17cdf 诊断版，见 [desktop-dmg-bdc17cdf-candidate.json](desktop-dmg-bdc17cdf-candidate.json) 和 [fresh-desktop-bdc17cdf-update.json](fresh-desktop-bdc17cdf-update.json)。首帧已 applied 但未触发重绘，仍在调查，不能据此计 S4 或 S5 通过；此前 0e866f5、ad2d915、3b017b1 和 508f325 仅作为历史 fresh 安装记录保留。
- Android：已同签名覆盖安装并保留应用数据的 `01f14d78f203456ca9c64d8483578fd02c66d68a` Release；恢复接管截图已实际查看，画面含用户、输入框、交还和 5–10 fps，见 [fresh-s4-android-restored-takeover.png](fresh-s4-android-restored-takeover.png)。无 note 的 native `takeover.release` UUID `4dfb02dd-6117-492c-b806-6c1067eabd56` 已 done（`18:26:31.605Z`），`record_after.state=done` 但 `browser_takeover_after=false`，原生对话框未自动关闭；root 手动点击关闭后实际截图 `fresh-s4-android-release-driver-bot.png` 显示 Bot 操作中、无输入、接管按钮和持续 fps，仍不能计及时或自动交还 UI，见 [fresh-s4-android-native-release-retry.json](fresh-s4-android-native-release-retry.json)。
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

1. 运行现有 `scripts/e2e/s1/`，再按 PLAN/DESIGN 顺序运行 S2、S3、S4；fresh S1 工具闭环和 kill-9 恢复已有证据，桌面 cache 已收到 50 段全文且 after-layout 图已实际查看。Android 恢复接管画面已实际查看，native release 服务端已 done 但 browser false，手动关闭后的截图仍显示 Bot 操作中、无输入、接管按钮和持续 fps，原生交还 UX 仍未闭环。server 30212bc 部署后首次 bootstrap I/O error5 的失败证据保留，确认无服务和无 7788 监听后重试成功；只读 bootstrap/workbench/routine 耗时已记录，另一次 warm bootstrap 为 8.071 秒；与 47fdd2a 的 30/37/14 秒点测相比仅表示本次观测改善，不是全面性能通过。fresh S2 已完成两个群并行和运行中同 assignment/run 插话；A 仍待验收未 confirm_done，B 两端 review 截图已查看但出现 request timeout，已在同 run 以修正 slug 成功发起 request_review，旧报告退出项 PARTIAL 保留。fresh S3 与 S4 仍为局部证据；桌面置前后仍为旧画面，Dock 工具 timeout，BetterDisplay occlusion 待用户 A/B 对照。47fdd2a 旧 preflight timeout 仅保留历史，不重跑安装或旧 preflight；旧持久 pending 仅保留在离线备份，不在 fresh 中续办旧群。
2. 每阶段保存桌面 `screencapture`、Android `adb exec-out screencap -p`、RPC/e2e JSON、source/PID 和实际断言；健康、API、mock 或单张 owner 图不能替代双端联合验收。
3. S5 通过条件是 pkg 安装、管理页设密码、两端连接、完整场景和截图全部完成。失败时保留新现场和证据；停止新 LaunchAgent 后，从仓库外备份恢复 `~/MacBot`、plist、对应 App 路径和 CLI links，恢复原外部 file secret 目录，Keychain 不变，再核对 source/PID/health。不要清空 Android 数据或密钥，也不要混用新旧数据。

当前状态：S5 已完成离线备份、3d6b289 native pkg 安装、管理页密码、file backend/模型配置以及两端安装连接；server `30212bc7282b1646ee817accab2b0ba6bd9eb2ea` 已部署 PID `4843`，health/setup 正常，首次 bootstrap I/O error5 与 service absent/7788 无监听失败边界保留，随后重试成功；jobs 哈希、pending、plist 未变化，系统 receipt 仍为 3d6b289。30212bc 只读 bootstrap/workbench/routine 已完成，耗时 `9.033/0.144/0.103s`；47fdd2a 的旧 preflight timeout 仅是历史快照。Android `01f14d7` native release 服务端已 done 但 browser false，手动关闭后截图仍显示 Bot 操作中、无输入、接管按钮和持续 fps，不能计及时或自动交还 UI。桌面 bdc17cdf 置前后仍为旧画面，Dock 工具 timeout，BetterDisplay occlusion 待用户对照；adapter 性能补丁未部署。S1 联合实时流式与性能仍待完成；S2 A 未 confirm_done，B request_review 已成功但旧报告退出项 PARTIAL；S4 schedule 晚 94.255847 秒，目标通知被旧 A 洪水挤出且打开未验收，X、桌面持续画面和联合验收待完成。设置密码前 `setup_required` 未观测，保留为证据缺口；本计划不构成验收通过。
