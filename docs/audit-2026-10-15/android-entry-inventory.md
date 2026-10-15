> 本文是修复前的只读入口盘点，表中的“待验证”保留为审查历史。M-06/M-07后续已修复，最终独立shared196/app37及APK构建通过；实际深链、旧Host接管释放、搜索和多选结果以[动态台账](../AUDIT-2026-10-15.md)及最终验收报告为准。未实测项不能据此升级为已验证。

# Android 页面入口 → RPC / 状态 / 事件审查清单

> 后续定点审查已将部分下述“待核实”升级为 M-06：无效 deep link 先切 Host 再校验资源，失败后保留原页面；Settings Provider/Model 与 Bot 新建表单可能向新 Host 提交，Trace 的 effect 和 unsubscribe 未完整绑定原 Host。普通手动切换会离开页面并取消其 scope，不能混为同一触发路径。M-06 正在独立批次修复，下文保留初步盘点与未覆盖范围。

审查范围：`clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile` 静态源码。没有运行设备或服务端场景；“静态接通”只表示代码中存在调用与状态读取/事件归并链路，不代表真实服务端运行验收。独立 QA 尚未结束，运行证据以后续验证者报告为准。当前代码保持冻结。

## 页面链路

| Feature / 页面入口 | RPC、HTTP 与输入 | 状态 / 事件 / 页面结果 | Host 绑定与覆盖状态 |
|---|---|---|---|
| connect / hosts | `ConnectScreen` 管理 Host 配置、连接、切换；`OnboardingTemplatesScreen` 显式对 `onboardingHostId` 调 `bot.templates`、`bot.create_from_template` | 首 Host 创建后进入 onboarding，再回主界面；Repository 按 Host 保存 session/state，`host.status` 更新连接状态 | 显式 Host 的 onboarding 静态接通；公网/局域网/远程 TLS 地址的真实 Android 拨号、断线切换尚未覆盖 |
| chat / MainBot / Messages | `chat.history`, `question.list`, `skill.list`, `chat.send`, `chat.mark_read`, `chat.react`, `approval.decide`, `question.answer`, `takeover.release`; 上传 `uploadFileOnHost`; 文件预览 `GET /api/v1/files` | `message.created/updated/deleted/delta`, `typing`, `read.updated`, question/approval/assignment/trace 等事件归并到 `MobileState`；聊天显示消息流、问题卡、附件和追踪入口 | 草稿、技能、回复、提及、附件、预览、pending answers 均 `remember(hostId,chatId)`；草稿和附件按 Host+chat 做持久化。上传/发送显式捕获 Host 并避免切 Host 后写入当前 UI。需实机验证切换 Host 时恢复草稿/附件及延迟上传；`chat.mark_read/react/approval.decide/takeover.release` 与若干历史读取使用当前 `repository.call`，闭包未捕获 request Host，属于待核实切换竞态。多选字段已连到 `question.answer`；失败保留选择、重复点击 guard 已静态实现，双入口实际失败/重试验收未覆盖 |
| approval / workbench / followup | Approval: `approval.list`, `workbench.get`, `approval.decide`, `question.answer`, 冲突时 `question.list`; Workbench: `workbench.get`, `chat.history`, `message.steer`, `assignment.retry/stop`; Followup: `workbench.get`, `approval.list`, `chat.history` | `approval.requested/resolved`, `question.asked/answered`, `assignment.created/updated`, `workbench` 由 RPC reducer 更新；入口跳聊天/审批/trace/group/computer | 均通过活动 Host 的 `repository.call`；问题卡现在多选和提交 guard 已接线。服务端冲突/过期与快速连点的 UI 实测未覆盖 |
| group / groupinfo / create group | `project.list/get/create/update/add_member/remove_member/confirm_done/request_changes/archive/reopen`, `announcement.update`; 消息操作沿用 Chat | `project.created/updated`, `announcement.updated`, `assignment.*`, 消息事件更新项目和聊天；组详情、公告、成员、artifact、trace 入口 | 活动 Host RPC，数据依赖 `project.*`/`announcement.updated` 事件；切换 Host 中途的编辑表单/旧响应隔离未覆盖 |
| bots / bot detail / editor | `bot.list/templates/get/create_from_template/create/update/duplicate/delete`, `project.list`, `assignment.list`, `usage.breakdown`; 头像上传 `uploadFile` | `bot.created/updated/deleted/status`、`assignment.*` 更新列表/详情；跳 chat/computer/routines/history/group/trace | 部分详情查询捕获 request Host；列表、模板、编辑/头像多为活动 Host。远程 Host 上传/切换期间编辑结果隔离未覆盖 |
| computer | 屏幕 WebSocket 由 `createScreen(botId,quality,tab)` 建立；`takeover.start/release` | 视频帧流更新画面/性能/标签；`host.status` 显示接管状态；连接的异常清理由 `LaunchedEffect(botId,quality,activeHostId)` 与 finally stop | screen effect 依赖活动 Host 并在 Host 变化时重建；takeover start/release 绑定 `ownerHostId`。画面、标签、接管跨 Host 切换是否无旧帧闪回需 AVD 实测，静态代码每次 effect 重置 frame/image/zoom 等 |
| trace / history assignments | `trace.history/subscribe/unsubscribe`, `assignment.list/stop/retry`, `message.steer`, chat history/list; `GET /api/v1/trace/output` | `trace.item/delta/tool_output`, assignment/message/usage 事件归并，追踪分片按 stream/request key 缓存；跳 computer / chat | 部分初始化显式 `activeHostId`，但订阅、翻页、stop/retry 和输出 fetch 使用活动 Host helper；Host 变化时 streamKey/effect 重跑，异步旧结果是否泄漏需设备验证。远程 Host trace HTTP 尚未覆盖 |
| dashboard | `usage.summary/heatmap/timeseries/breakdown`, `GET /api/v1/usage/export` | RPC 返回值填充图表/导出 CSV，无对应逐项 reducer 事件 | 活动 Host；只静态确认 URL helper 按 Host profile 组装，远程路径与导出未实测 |
| skills | `skill.list/get/update/delete/set_enabled/import`; upload `uploadFile` 后 `skill.import` | `skill.updated/deleted` 与 RPC 结果刷新列表/详情 | 活动 Host；导入上传、切 Host 中的对话框和远程 HTTP 尚未覆盖 |
| routines | `routine.list/create/update/set_enabled/delete/test_run/runs` | `routine.updated/deleted/run` 更新 state 和最近执行；跳 `trace` | 活动 Host；结果/错误有页面处理，真实调度与服务端执行链路未在本轮验证 |
| search | `search(query...)`；结果跳 bot editor / routines / file / chat | 结果由 RPC 临时呈现，artifact 结果通过 `project.list/get` 查公告 artifact，消息结果解析 chat target | 活动 Host 的多个顺序 RPC；分页/空态/远程 Host search 实测未覆盖 |
| memory / profile editor | 显式 `callOnHost(hostId, memory.user.list/create/update/delete, memory.profile.get/update)`，版本冲突时重新读；可跳来源聊天 | `memory.user.updated/deleted`, `memory.profile.updated` 归并记忆与 quota/profile；只有用户记忆页面 | 静态 Host 显式绑定较完整；并发版本冲突页面链路已实现，服务端冲突现场/切换 Host 未覆盖 |
| settings / approval rules | `settings.get`, `provider.list`, `host.permissions`, provider/model CRUD/test/refresh；规则编辑显式捕获 Host 调 `settings.update` | `settings.updated`, `provider.updated/deleted` 更新设置、供应商和模型 | 普通设置是活动 Host；规则保存是显式 requestHost。Host 切换时表单仍保留旧 Host 字段的 UX/提交隔离未覆盖 |

## 跨页面底层链路

- `App.kt` 将 route/page 分发到上述页面；通知事件 collector 仅处理 `hostEvent.hostId == activeHost.id`，避免非活动 Host 通知混入当前 UI。深链、trace 返回锚点保存 Host id 并在返回时比较。
- `ClientRepository` 为 Host 独立维护 session、MobileState、epoch、持久化 snapshot 与序列游标。`callOnHost` 在 request-start 取 state/epoch fence，RPC 响应与期间事件/tombstone 做 reconciliation；Host 切换后只 publish 当前 Host。WebSocket 主连接严格连续 seq 和 `sync.done` barrier。本轮之前有相应 host integration/repository 测试，自身测试通过不等于外部独立验收。
- State reducer 已静态映射 chat/message/read/typing、bot/project/announcement/assignment/approval/question/skill/routine/memory/settings/provider/usage/trace 事件。未知事件走忽略分支。RPC reducer 对 list/get/history/workbench/trace 等响应更新快照或合并实体。
- 网络端点并非硬编码 localhost：Repository 根据 Host profile session 构造 RPC/HTTP/WebSocket 地址；电脑端实时画面走独立 screen connection。对远程域名、TLS、自签证书、代理和公网断线恢复，本次只有源码检查，没有 Android 设备连远程 Host 的证据。

## 需纳入独立验收的重点

| 检查项 | 静态结论 | 状态 |
|---|---|---|
| Host 切换后聊天草稿/附件隔离 | key 为 `(hostId, chatId)`，picker/upload/send 捕获 Host；发送成功清空前检查 Host 仍匹配 | 静态接通；需双 Host AVD 实测 draft/attachment 恢复、切换瞬间上传/发送晚返回不清空新 Host 内容 |
| Host 切换后的 trace/电脑画面隔离 | trace 局部 state 多数以 activeHostId 为 key；screen effect 以 activeHostId 重建并清空帧，takeover 使用 ownerHostId | 静态接通但有异步旧回调风险待实测；验证旧帧/旧 trace/output 不显示到新 Host，旧 Host takeover release 仍发回 owner Host |
| Host 切换后的普通 RPC/HTTP 操作 | 多个页面在主动动作中调用 `repository.call`（当前活动 Host），与部分显式 Host 调用并存 | 待验收竞态；重点 chat mark-read/react/approval/takeover、trace stop/retry/output、settings 表单和 bots 上传。源码没有足够证据认定实际错误，暂列未确认风险 |
| 多选问题回答 | Chat 与 Approval 的 choices 都应发送 `option_indexes`; approval/chat 已有逐卡 in-flight 禁用，失败不清空选择，冲突重新拉问题 | 静态链路已接；需在真实 MockWebServer 错误响应下双入口验证快速双击仅一请求、失败显示错误且选项保留、重试成功、已回答/过期不重复触发 |
| 请求 ID 与副作用去重 | Repository 对写 RPC 租用稳定 `client_request_id`；完成推迟到 response fence/reconcile/persist 完成后 | 新增真实连接时序回归测试：服务端先记成功，bootstrap 变化再放开旧 response，旧调用取消后重试沿用原 ID且 effect 计数为一。当前独立 QA 需复跑这条测试 |
| 远程 Host 全链路 | host profile 驱动 RPC/HTTP/WS base address，页面存在显式 Host 与活动 Host 两类调用 | 仅静态确认；未覆盖远程 TLS host 的 login/bootstrap、同步 gap/reconnect、chat send/upload、screen 和 usage/file/trace HTTP |

## 已知状态 / 覆盖边界

- 静态接通：上述 feature 页面对应的 RPC/HTTP 调用、repository state、事件 reducer 与路由入口均有源码路径。
- 本任务只读，没有重复 Gradle；最新自测 Build 与 188 tests 通过的结论来自之前的工作记录，独立验证尚在进行中，应以 `mobile_verify_luna` 的最终 XML、APK 和 AVD/mock 报告为准。
- 未覆盖：真实公网/远程 Host、双 Host 并发切换、所有页面的服务端事件响应、全页面 Android UI/无障碍、附件重试/持久化损坏恢复、问答失败和过期现场、多 Host trace/output 和屏幕帧隔离。
- 本次审查未新增“确认缺陷”。当前待验收风险不应等同于可复现缺陷；此前识别的 write receipt 与 multi-question in-flight 问题已有冻结修复，仍须独立验证。
