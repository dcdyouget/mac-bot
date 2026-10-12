# 主工作树整合版验收

- 时间：2026-10-12 14:35–14:45 CST
- APK：主工作树 `androidApp-debug.apk`，SHA-256 `5c362461c8dc56dd959a129b69d0033677881289bd6ccf76c67452e5bfcf13f2`；安装时间 `2026-10-12 14:35:04`。
- 设备：独立 AVD `macbot_accept_batch01_api36`，`emulator-5558`，Android 16 / API 36。未使用 longpress 的 emulator-5564。
- 服务：`macbotd --mock --port 17937 --password dev`，数据目录 `/tmp/macbot-mobile-skill-probe-integrated-20261012`，Android 地址 `10.0.2.2:17937`。Health 返回 `ok=true, protocol=1, setup_required=false`。

## 构建与测试

- `ANDROID_HOME=/Users/gongshaojie/Library/Android/sdk ./gradlew :shared:commonTest :shared:androidUnitTest :androidApp:testDebugUnitTest`：`BUILD SUCCESSFUL`；测试任务为 Gradle `UP-TO-DATE` / `FROM-CACHE`。
- `ANDROID_HOME=/Users/gongshaojie/Library/Android/sdk ./gradlew :androidApp:assembleDebug`：`BUILD SUCCESSFUL`，产物即上列 SHA 的 APK。

## GUI 与 RPC

基线首次连接约 14:38:58 出现 `MainConnection diagnostic stage=ready http_status=none exception_class=none`。完成 onboarding 后进入 `官网改版` 群聊。浅色与深色模式均实际完成：输入 `/web` 显示 `/web-auth-patterns` 候选，选择后 composer 出现 `web-auth-patterns` chip；输入文字、打开「提及成员」并选择「编码」，文本编辑区显示 `@编码`；发送后消息在群聊中可见。

对 `chat.history(chat_web)` 的 RPC 使用 `Authorization: Bearer dev`。浅色和深色的结果都包含 `kind=bot, bot_id=bot_code, instruction=null` mention。浅色正文 `/web-auth-patterns Review the login flow @编码`；深色正文 `/web-auth-patterns Dark theme skill test@编码`。RPC 原文保存在 `chat-history-light.json`、`chat-history-dark.json`。首次误用 HTTP Basic auth 得到 401，改用 Bearer 后成功，不影响 GUI 发送验证。

## 截图

- 浅色：`light-web-candidate.png`、`light-skill-chip.png`、`light-mention-picker.png`、`light-mention-selected.png`、`light-sent.png`。
- 深色：`dark-web-candidate.png`、`dark-skill-chip.png`、`dark-mention-picker.png`、`dark-mention-selected.png`、`dark-sent.png`。
- `connected.png` 为首次 ready 后 onboarding 页面，`after-onboarding.png` 为主 Bot 页面，`webchat.png` 为群聊初始页面。

未修改产品代码，未提交。此轮只覆盖浅色、深色；半透明模式不在本次 verifier 指令范围内。
