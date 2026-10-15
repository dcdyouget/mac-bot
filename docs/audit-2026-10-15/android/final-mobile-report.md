# Mobile 独立验收（capacitytest）— 2026-10-15

## 构建与自动化测试

M06/M07 冻结后独占运行 `./gradlew --no-daemon --rerun-tasks -PmacbotNotificationCapacityIsolated=true :shared:testAndroidHostTest :androidApp:testDebugUnitTest :androidApp:assembleDebug :androidApp:assembleDebugAndroidTest`，`BUILD SUCCESSFUL`。归档 `final-m06m07-xml/` 共 42 个 XML、196 tests，`final-m06m07-app-xml/` 共 6 个 XML、37 tests，失败/错误/跳过均为 0。产物是 `clients/mobile/androidApp/build/outputs/apk/debug/androidApp-debug.apk`，applicationId 为 `bot.mac.mobile.capacitytest`；只安装在隔离 AVD `macbot_audit_mobile_20261015`（`emulator-5584`），未操作 `bot.mac.mobile` 或 `macbot_api36`。

## 最终 APK 实测

- **搜索回归**：Messages → 搜索 `login` → 点精确结果“登录功能”可打开项目群聊。最终 APK 保持在 `MainActivity`，logcat 未见 fatal、ANR 或 force-finish；证据 `final-search-open.png/xml`、`final-search-open-logcat.txt`。旧 APK 曾以完全相同步骤复现 Compose 重复 LazyColumn day key 崩溃，栈与进程退出证据保留在 `search-accurate-logcat.txt`、`search-accurate-activities.txt`；早先“无崩溃”报告是误触偏移后的错误结论，已用精确坐标更正。
- **聊天多选失败**：`chat_login` 第 3 题选 `[0,2]` 后双击提交，失败后两项仍选中且错误可见；即时响应 proxy 记录一条 `question.answer`。证据 `final-chat-multi-selected.png`、`final-chat-multi-failure.png/xml`、`mobile-rpc-final.jsonl`。
- **审批多选失败与在途双击复测**：审批页 `q_mock_login_flow_3` 选 `[0,2]`，对失败响应即时返回时两次 tap 间隔约 59ms；日志中第一条请求及其 conflict 响应先于第二次请求，因此第二条是失败后的合法手动重试，同 `client_request_id` 符合预期，不能据此判定并发重复提交。随后 proxy 延迟 500ms 后才返回 conflict；首请求在途约 100ms 时再次 tap，最终仅出现一条 `question.answer`，随后在约 500ms 失败后触发一次 `question.list`。失败后 UI 保留两项并显示错误。证据 `approval-500ms-selected-before.png`、`approval-500ms-after.png/xml`、`mobile-rpc-delayed.jsonl`。这验证的是客户端失败状态及 mock/proxy 下的请求时序，不验证真实服务端权威过期状态。
- **跨 Host 深链与表单清理**：新建独立 Host B（`127.0.0.1:18791`）后，A、B 同时连接。有效 `chat_main` 深链打开目标 Host 的聊天。A 的新建 Bot 表单输入草稿后，打开指向 B、不存在 Bot 的 `macbot://computer/no_such_bot?host_id=...`，应用切到 B 并回到 home/My 路由，旧表单退出；证据 `final-bot-form-draft-a.png`、`final-invalid-link-bot-form-cleared.png/xml`。`final-valid-link-a.png/xml` 和 `final-invalid-link-bconnected.png/xml` 留有有效/无效链接页面。Provider 表单未单独重复此 UI 流程。
- **旧 Host takeover release**：A 上实际发出 `takeover.start(bot_main)`，再通过深链切到 B；A RPC 日志记录 `takeover.release(bot_main)` 成功，B 日志没有 release 请求。证据仅为 `mobile-rpc-final.jsonl` 与 `mobile-rpc-host-b.jsonl`；没有保存 takeover 专用截图/XML。
- **Trace unsubscribe 归属**：UI入口为 Messages → Main Bot 聊天中的“已委派给 bot_product：快速调研”消息 → 滚动消息列表，使“详情”按钮移出底部输入框遮挡区域 → 点“详情”，进入 `asg_mock_delegation` Trace 页面。mock原始 `trace.history` 固定返回 `live=false`；为进入客户端真实订阅生命周期，临时代理只把该响应的 `live` 改为 `true`，原始 items/aseq 保持不变。页面显示“● 实时”，A 收到 `trace.subscribe`；切换至 B 的有效 `chat_main` 深链后，A 收到对该 stream 的 `trace.unsubscribe`，B 没有收到 unsubscribe。证据 `trace-live-shim.png/xml`、`trace-after-host-switch.png/xml`、`mobile-rpc-trace-shim.jsonl`、`mobile-rpc-host-b-final.jsonl`。该结果覆盖客户端 Host 捕获/清理路径，不证明 mock 有真实 live trace 数据或调度行为。

## 环境与边界

两个隔离 mock 使用独立 `--config`、`--home`、端口和密码。初次 B 握手失败是复用了带历史 password hash 的临时 home；使用新建 `mock-home-b-final` 后连接成功。所有验收用 mock/proxy 均为本次临时进程，结束时已停止。日志 JSONL 只包含 RPC 方法、参数、请求 id、时间及结果，不含 Authorization 或密码。

本验收能证明最终客户端在 mock 下的 UI、请求目标及状态行为，不能证明真实任务调度、Bot 执行或服务端实际业务结果。ApprovalScreen 将非 `pending` 状态排除在待处理列表外，符合该页面的待处理语义；未知未来状态的显示不作为本轮缺陷。

完整原始日志、XML与其余截图保存在 `/tmp/macbot-audit-20261015-iEFvBx/mobile-verification/`；本目录归档报告、无凭据摘要和关键截图。
