# S5 全新安装演练

状态：待 S1–S4 真实联合验收完成后执行。本文件是操作计划，不是通过记录。

1. 固定 server-mac 声明 ready 的完整 SHA 及对应 pkg、client-mac DMG、Android 签名 Release；分别核对 source-commit、dirty=false、SHA-256。pkg 必须来自完整正式 composition，不能使用早期 packaging-only 包。
2. 记录 LaunchAgent/listen PID、实际可执行路径、两客户端版本；暂停 watcher，停止 `com.macbot.server`。备份服务数据、访问密码及两端当前连接配置到仓库外；保留共享 mock 和模拟器用户数据。将原 `~/MacBot` 移到备份位置，避免把升级误认为全新安装。
3. 按 server README 卸载 LaunchAgent 和旧 App/CLI 链接。注意开发版 `~/Applications/MacBotServer.app` 与 pkg 的 `~/Applications/MacBot Server.app` 路径不同、使用同一 label；检查 7788 已释放，不能只看卸载命令返回值。
4. 安装固定 pkg；核对包内 daemon/官方 sidecar/许可证、安装路径及 plist。`status.sh` 应报告实际运行 binary/source SHA，LaunchAgent PID 必须与 7788 listener 一致。
5. 确认干净 home 的 `/api/v1/health` 返回 `setup_required=true`，通过 `/admin` 完成首次密码设置，再以认证 RPC 检查 bootstrap。访问密码仅存本机 `~/.macbot-dev-password`，不放入截图或提交。若使用 UI 设置新密码，密码输入、确认、提交交由用户完成。
6. 当前开发期沿用用户授权的 file secrets：显式配置 `MACBOT_SECRET_BACKEND=file` 和仓库外 secret dir，再配置 MiniMax provider/default models。pkg 默认 Keychain，不能假设开发 plist 环境会被安装器保留；两种环境的证据需写清楚。不得把旧业务数据恢复到本次 fresh-install home 后继续宣称全新安装通过。
7. 用正式 DMG 安装桌面 App，Android 用相同发布签名更新；桌面连接 `127.0.0.1:7788`，模拟器连接 `10.0.2.2:7788`。完成 PLAN 第 6 章和 DESIGN 第 3 章场景，截图桌面与 Android，记录每个实际 assertion。仅健康、API 或 mock 截图不够。
8. 验收后保留可用安装版；记录备份位置、最终版本/PID、密码文件位置、阶段结论、截图和遗留问题。失败时先保留现场，再按记录恢复旧服务数据/安装路径，不清空 Android 数据或密钥。

当前只读候选证据：[22b3 pkg](server-pkg-22b3b10-verified.json)、[桌面 DMG](desktop-dmg-222fda9-candidate.json)。这些尚未作为一次完整演练安装运行。
