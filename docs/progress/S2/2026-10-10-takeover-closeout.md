# 2026-10-10 接手续办记录

仍只有 S0 整阶段通过；本记录中的代码回归、API 和截图是局部证据，不提升 S1–S5 的结论。沿用 dev/integrator、原项目和 MiniMax 配置；主仓库未跟踪 design/ 未动。用户确认解锁后已恢复桌面访问。

## 已修复并验证

- dcd5e36：公告从当前工作中/待处理/阻塞任务选取状态，避免 HashMap 顺序随机选到旧 done。orchestrator 57 测试及 clippy 通过，正式 API 和桌面观察通过。见 announcement-selection-before/after、desktop-announcement.png。
- ff083bc：browser_nav 正确保留 reload/back/forward，不再把动作一律转为 open。gateway 187 测试及 clippy 通过；原 Tester aseq75 真正 reload 成功。ff083bc 升级前后的聚合文件 hash 断言失败，不能声称请求/job 字节不变；单次审批使用独立 checkpoint/args/identity 校验通过后执行，详见 upgrade-after.json。
- cc8e6d0：browser_wait(timeout/selector) 和 browser_get(property,selector) 正确映射参数，补 schema/工具描述。gateway 187 测试及 clippy 通过；原 Tester aseq104 返回 waited=timeout, ms=1000。
- b7d6c76：同一 run 的审批续跑与恢复调度共享执行互斥，避免并发调用模型和工具。gateway 187 测试及 clippy 通过；7fa39b0 为测试等待增加五秒上限，无业务差异。
- 487122a：桌面轨迹采用 GPUI 原生变高虚拟列表，绘制前测量行高，追加/前插保留滚动位置。86 个客户端测试及严格 clippy 通过；真实 TraceView 的 360/720px 首帧、展开及内容更新布局断言通过。8d347b6 补行宽约束后，实际窄栏截图已确认正文换行、行间不覆盖（production-8d347b6-desktop-trace-after.png）。原重叠截图见 production-cc8e6d0-desktop-trace-overlap-before.png。

- 2f09174：聊天消息同样改为绘制前测量，保留历史/待发消息顺序和追加/前插锚点；历史按钮和消息列表共用纵向布局，避免侵入输入区。86 测试与严格 clippy 通过；实际长消息滚动、窄窗口及“跟随最新消息”通过，截图 production-2f09174-desktop-chat-{after,narrow}.png。

## 真实 Tester 的失败与续办

原任务 01a12365-0d0f-7514-b461-ec9aa2006a89 实际执行了浏览器打开、输入和快照；无效邮箱的浏览器校验可见。升级恢复后输入被清空，已向原 run 提供纠正反馈，不将空输入结果当作错误密码通过。

原 run 的 aseq105/106 同时出现模型第15/14轮请求，随后 aseq108 snapshot 启动、aseq109 等待审批、aseq110 HTTP400失败。并发运行是确定的平台缺陷；由于未保存 provider 错误响应体，不能断言 HTTP400 的具体原因。原失败 run/job/trace 不手改、不补造 tool.end。

主 Bot 随后自动创建续办任务。b7d6c76 部署后的用户续测消息 UUID ddbbff26-ec3d-4ea2-abda-c4f99c7afd30、seq34 被送入既有 Tester 01a1237c-a9d4-75cd-89f9-39689ebdd095，delivery=read；不是新建项目或重复历史请求。该任务已实际读取原 HTML 并打开 t3，最终 aseq99 因工具/模型轮次上限失败，失败证据保留。第三次续办复用 Main 已建立的 Tester 01a12391-e424-75cb-8cab-255f083fa287、tab t4；未调用的函数表达式返回空对象，不算通过，已在同任务追加纠正反馈。其他自动产生的 siblings/pending 未批量取消或批准。

所有写 RPC 先记录 UUID；审批逐次核对实际 args、map、checkpoint、run/asg/Bot/project/chat，仅 allow_once。仅操作本机 52205 的 demo 页面，不手改 demo、不放宽 Bash/subagent/全局 memory。旧 TEST.md 未实际浏览器测试的失败边界仍保留；TEST-browser.md 尚未完成。

## 最短剩余清单

1. 完成原两群 Tester 实测、双群并行和主 Bot 汇总到待验收；不要提前 confirm_done。
2. 桌面列表重叠已修复并完成原生专项；S1 两端真实流式/实时轨迹/回放与 kill-9 恢复。
3. S3 同一时间范围两端用量、技能、仪表盘一致性。桌面 2026-10-09 UTC 已实际显示 5,703,826 tokens / 540 requests，与 RPC 一致；Android 待同范围复核。
4. S4 桌面持续画面及自动/低清切换已局部通过（见 S4/production-2f09174-desktop-screen.json）；接管输入、X 登录、通知容量及动作仍待。Android 模拟器不能绑定为 CUA 原生窗口，adb 输入方式的确认待用户回应。
5. 最后执行 fresh-install-plan 的可逆备份、pkg、管理页设密码、两端连接与完整场景。当前未执行 fresh；旧 pkg 不能替代当前源码安装验收。

## Android 网络恢复

模拟器 wlan0 无载波且路由为空，导致客户端重连；重新连接已保存 AndroidWifi 后，10.0.2.2:7788 health 返回 200，客户端自动显示后续消息。未重启、清数据或更换 APK，不归因为客户端缺陷。见 production-b7d6c76-android-network.json 和前后实际截图。
