# DECISIONS.md：决定记录

> 只记仍然有效的产品和技术决定，一条一行，新的写在最上面。被推翻的决定直接改写或删除，并在提交说明里写明。2026-10-11 之前的多开发线协调记录见 `docs/archive/COORDINATION-2026-10-09_11.md`。

## 开发方式
- [2026-10-11] 一个主 agent 负责全部代码，按模块派 subagent；只保留 `main` 一条长期分支；不再设集成线（AGENTS.md）。
- [2026-10-11] macOS 桌面客户端界面层从 GPUI 迁移到 Tauri 2 + React；`macbot-client-core`（网络层、数据层）不变。
- [2026-10-09] 服务端使用 JSON 文件存储，不引入数据库（PLAN 5.2）。

## 产品与界面
- [2026-10-11] 审批模式只有两档，默认「自动允许」（只有命中「先问我」规则才审批）；另一档「先问我」。审批规则带作用范围（某个 Bot 或所有 Bot），内置规则只读。
- [2026-10-11] 待看消息：Bot 交付前由服务端一次性交给它查看（queue.review），任务带 `pending_messages` 计数。
- [2026-10-11] Bot 在消息里提到某个群时可以附群聊卡片：`send_msg.projects` → `project_card(context = mentioned)`。
- [2026-10-11] 「设置 → 电脑与浏览器」显示 Bot 主机的系统权限：`host.permissions`。
- [2026-10-11] 只有 @ 某个 Bot 或回复它的消息才打断它（下一步生效）；其余群消息进入它的「待看消息」，交付前一起查看（delivery.state = queued）。
- [2026-10-11] 右键消息可以「回复」（交给原发送者）或「引用」（不指定谁回应）：`chat.send.quote_mode = reply | reference`。
- [2026-10-11] 三端唯一提示音：`design/sounds/04-bubble-pop`（啵啵 · 气泡）。
- [2026-10-10] 不做线程，所有沟通都在群里；`reply_to`、`thread_count`、`chat.thread` 不再使用。
- [2026-10-10] 公告是一段自由文本（Markdown 显示），Bot 用 `update_announcement` 工具、用户用 `announcement.update` 修改；没有「流程」等固定字段。
- [2026-10-10] 一条消息里的多个 `question` 块合成堆叠卡片；选项在 Bot 的消息里，回答后原地变成已回答的样子。
- [2026-10-10] 主 Bot 戴黑框眼镜（秘书形象）；Android 与 iOS 外观完全一致；三端图标统一用 Lucide。
- [2026-10-10] 界面视觉、像素、文案 key 以 `design/` 为准；DESIGN.md 只管产品行为。
- [2026-10-10] 手机端功能与桌面一致（包括完整编辑定时任务），只是三栏拆成页面栈。

## 待处理
以下规范已写进 PROTOCOL / PLAN / design，代码尚未实现（需在同一个提交里改完协议、服务端、mock、两端）：
- 待看消息交付前查看：`Assignment.pending_messages`、轨迹 `queue.review`、`send_msg(done)` 前拦截并交给 Bot、delivery 变为 read（PLAN 5.3.4）。
- `send_msg.projects` → `project_card(context = mentioned)`（PLAN 5.3.2）。
- `ApprovalRule.bot_id`、`builtin`；审批模式默认值改为 `always_allow`，内置规则只读。
- `host.permissions` 方法（屏幕录制、辅助功能、完全磁盘访问、Chrome 自动化）。
- server：`macbot-orchestrator` 的 `state::tests::approval_and_question_waiting_states_validate_inputs` 失败（Conflict("persistent allow is not supported for this approval")），合入前就存在。
