# 2026-10-10 接手续办记录

仍只有 S0 整阶段通过；本记录中的代码回归、API 和截图是局部证据，不提升 S1–S5 的结论。沿用 dev/integrator、原项目和 MiniMax 配置；主仓库未跟踪 design/ 未动。用户确认解锁后已恢复桌面访问。

## 已修复并验证

- dcd5e36：公告从当前工作中/待处理/阻塞任务选取状态，避免 HashMap 顺序随机选到旧 done。orchestrator 57 测试及 clippy 通过，正式 API 和桌面观察通过。见 announcement-selection-before/after、desktop-announcement.png。
- ff083bc：browser_nav 正确保留 reload/back/forward，不再把动作一律转为 open。gateway 187 测试及 clippy 通过；原 Tester aseq75 真正 reload 成功。ff083bc 升级前后的聚合文件 hash 断言失败，不能声称请求/job 字节不变；单次审批使用独立 checkpoint/args/identity 校验通过后执行，详见 upgrade-after.json。
- cc8e6d0：browser_wait(timeout/selector) 和 browser_get(property,selector) 正确映射参数，补 schema/工具描述。gateway 187 测试及 clippy 通过；原 Tester aseq104 返回 waited=timeout, ms=1000。
- b7d6c76：同一 run 的审批续跑与恢复调度共享执行互斥，避免并发调用模型和工具。gateway 187 测试及 clippy 通过；7fa39b0 为测试等待增加五秒上限，无业务差异。
- 487122a：桌面轨迹采用 GPUI 原生变高虚拟列表，绘制前测量行高，追加/前插保留滚动位置。86 个客户端测试及严格 clippy 通过；真实 TraceView 的 360/720px 首帧、展开及内容更新布局断言通过。8d347b6 补行宽约束后，实际窄栏截图已确认正文换行、行间不覆盖（production-8d347b6-desktop-trace-after.png）。原重叠截图见 production-cc8e6d0-desktop-trace-overlap-before.png。

- 2f09174：聊天消息同样改为绘制前测量，保留历史/待发消息顺序和追加/前插锚点；历史按钮和消息列表共用纵向布局，避免侵入输入区。86 测试与严格 clippy 通过；实际长消息滚动、窄窗口及“跟随最新消息”通过，截图 production-2f09174-desktop-chat-{after,narrow}.png。

- d627486：项目关注提醒每轮只扫描一次事件日志，修复每条历史提醒重复全量解析、长时间持有 RPC 写锁的问题。187 gateway 测试及严格 clippy 通过；升级后 approval.list 约 1.83 秒，16 个 pending 及当前 Tester job 哈希未变。前测发生在双 release 构建高负载期间，不据此宣称精确性能倍数。

- e78f048：浏览器命令保留数值/布尔参数，拒绝 null/对象/数组而不静默丢弃。187 gateway + 最终 10 工具专项及 clippy 通过。原 360×800 审批在部署后执行时遇到独立的恢复缺陷，不能算窄屏通过。
- 0d7c887：所有执行与续跑入口先应用 Bot 持久浏览器配置，避免审批恢复使用没有 state_path 的默认配置；缺标签错误会清除 busy 标记。188 gateway、17 browser 测试及 clippy 通过；已仅部署 server，未打开 Tester 屏幕，原审批 aseq92 实际返回 viewport=360×800；16个pending、job/run request/browser session原哈希保持。

## 真实 Tester 的失败与续办

原任务 01a12365-0d0f-7514-b461-ec9aa2006a89 实际执行了浏览器打开、输入和快照；无效邮箱的浏览器校验可见。升级恢复后输入被清空，已向原 run 提供纠正反馈，不将空输入结果当作错误密码通过。

原 run 的 aseq105/106 同时出现模型第15/14轮请求，随后 aseq108 snapshot 启动、aseq109 等待审批、aseq110 HTTP400失败。并发运行是确定的平台缺陷；由于未保存 provider 错误响应体，不能断言 HTTP400 的具体原因。原失败 run/job/trace 不手改、不补造 tool.end。

主 Bot 随后自动创建续办任务。b7d6c76 部署后的用户续测消息 UUID ddbbff26-ec3d-4ea2-abda-c4f99c7afd30、seq34 被送入既有 Tester 01a1237c-a9d4-75cd-89f9-39689ebdd095，delivery=read；不是新建项目或重复历史请求。该任务已实际读取原 HTML 并打开 t3，最终 aseq99 因工具/模型轮次上限失败，失败证据保留。第三次续办复用 Main 已建立的 Tester 01a12391-e424-75cb-8cab-255f083fa287、tab t4；未调用的函数表达式返回空对象，不算通过，已在同任务追加纠正反馈。其他自动产生的 siblings/pending 未批量取消或批准。

所有写 RPC 先记录 UUID；审批逐次核对实际 args、map、checkpoint、run/asg/Bot/project/chat，仅 allow_once。仅操作本机 52205 的 demo 页面，不手改 demo、不放宽 Bash/subagent/全局 memory。旧 TEST.md 未实际浏览器测试的失败边界仍保留；TEST-browser.md 尚未完成；第三次 run 在 aseq101 因16轮上限失败，aseq100 的 DOMRect={} 不算边界通过。剩余退出/数值边界/报告插话进入既有 Tester 01a1237c-7a83-75fb-bf4d-a209b7f1bb49（queued），没有新任务。第三次实测已观察空输入错误提示、错误密码保持未登录、正确登录成功、刷新后仍显示用户且存储保持；aseq85 在 server 重启后的续跑报 no tab for assignment，磁盘仍保留正确 t4 归属。

当前续办任务已实测：aseq62 返回 innerWidth=scrollWidth=360，但选择到隐藏容器、边界全0，不能计边界通过；aseq69 实际点击退出，aseq83 返回 loginFormVisible=true、userInfoVisible=false、localStorage=null。同任务aseq91已取得可见登录表单x=20、width=320、right=340、scrollWidth=360，窄屏表单通过。aseq94报告草稿把源码当实测、把错误密码拒绝当失败，纠正反馈后仅拒绝该write；aseq96按拒绝语义cancelled，报告尚未生成。新证据和修正要求已送入本项目Main跟进，旧失败与demo保持。搜索默认返回内部object/checkpoint已在63c140d修复，188 gateway测试及clippy通过；仅部署server，PID21455，旧pending/等待checkpoint保持，同查询API与原生截图确认只剩Bot和chat。

## 最短剩余清单

1. 完成原两群 Tester 实测、双群并行和主 Bot 汇总到待验收；不要提前 confirm_done。
2. 桌面列表重叠已修复并完成原生专项；S1 两端真实流式/实时轨迹/回放与 kill-9 恢复。
3. S3 同一时间范围两端用量、技能、仪表盘一致性。桌面 2026-10-09 UTC 已实际显示 5,703,826 tokens / 540 requests，与 RPC 一致；Android 待同范围复核。
4. S4 桌面持续画面及自动/低清切换已局部通过（见 S4/production-2f09174-desktop-screen.json）；桌面真实鼠标/键盘接管输入及释放已通过（S4/production-e78f048-native-input.json；切换控制使用记账 RPC，输入使用 CUA 原生窗口）。X 登录、Android 同场景、通知容量及动作仍待。Android 模拟器不能绑定为 CUA 原生窗口，adb 输入方式的确认待用户回应。
5. 最后执行 fresh-install-plan 的可逆备份、pkg、管理页设密码、两端连接与完整场景。当前未执行 fresh；旧 pkg 不能替代当前源码安装验收。

## Android 网络恢复

模拟器 wlan0 无载波且路由为空，导致客户端重连；重新连接已保存 AndroidWifi 后，10.0.2.2:7788 health 返回 200，客户端自动显示后续消息。未重启、清数据或更换 APK，不归因为客户端缺陷。见 production-b7d6c76-android-network.json 和前后实际截图。

## 07a3245 实时游标修复与两群 Tester 交付

- 现场发现桌面cache停在seq4952、下一条4953为内部trace.item。服务端持久全局seq被轨迹占用，实时却只向订阅者推无seq轨迹，导致普通连接永远缓存后续状态。8c7fe41新增sync.cursor契约；07a3245在实时与resume中投影同序号、无正文的占位帧，订阅轨迹仍独立按aseq发送。Rust/schema/fixtures/Kotlin同步，190 gateway（含真实WebSocket）、协议13测试、桌面核心34测试和Kotlin生成5测试通过。
- 仅部署07a3245，PID34089；全部原pending与等待checkpoint、first run request、Tester浏览器session哈希保持。桌面未重装、未点刷新，自动追上review与Tester完成。标准库正式WebSocket复验从4952补发，143个replay cursor、8个live cursor，无缺号、无未订阅轨迹正文；证据S1/production-07a3245-*。
- parallel报告旧草稿拒绝后终止。Main只发progress未带mentions没有派任务，已记录模型交接不足；明确的新报告纠正请求在原群生成report-only任务…4dc2。原Tester写报告并按真实反馈自行edit修正“空密码”措辞，send_msg done明确mentions main。报告保留无效邮箱证据不足、错误提示/alert未测、已登录卡片边界未测。Main经反馈纠正“验收完成”的错误标题并真正request_review，project=review；没有confirm_done。
- first旧待审批edit明确指向原login.html，与保留原失败要求冲突；核验map/checkpoint/身份后仅deny该已知旧edit，没有批量取消siblings。index.html哈希与既有独立浏览器证据一致，恢复52204本地服务。新实际Tester任务…9654：aseq24批量空输入/无效邮箱/空密码/错密码/正确登录；31真实reload；38登录保持、用户卡片x20/w335/right355且375px无横溢；45真实退出、存储null、表单可见且边界正确；56写TEST-index-browser.md，59向Main提交done，61结束。两份demo哈希保持，报告由原Bot生成；first Main已汇总并置review，主私聊待验收卡含index.html与TEST-index-browser.md，未确认完成。
- 最新07a3245 pkg已审计并存仓库外候选目录，未安装。旧63c140d候选保留为历史；无fresh通过声明。Android/X控制外部条件仍待，S0仍唯一整阶段通过。

- 两群最终公告与主私聊验收卡已核验，截图production-07a3245-first-group-review.png、production-07a3245-main-review-cards.png均实际查看。parallel登记2项；first重复登记同2个路径为4条记录，卡片仍为2个正确链接，保留模型重复操作。first Tester与parallel Main运行重叠112.033秒，见production-07a3245-two-project-run-overlap.json；仅证明两群实际run重叠，不代替双端并行验收。

## S1 同run恢复与实时轨迹缺口（2026-10-10续）

- 复用空闲画面联调-9e70私聊Bot，UUID先落盘；精确核验Bash、cwd/目标、approval-map、run request与waiting checkpoint后单次批准。Bash成功写入唯一marker，safe checkpoint落盘且尚未read时，仅kill核验的正式PID34089。LaunchAgent启动43442，同run在aseq57恢复，60/61实际read原文件，64 done；Bash未重放，marker哈希不变，桌面自动显示最终回复。证据S1/production-07a3245-owned-recovery.json。
- 实际桌面轨迹停在重连拉取的aseq57，后续read/结束未实时追加；截图保留，不能计实时轨迹通过。根因是production trace.history固定live=false，两端按协议不订阅。7f0a5dd改为分页前按全部run起止判断live，等待/恢复和父子run均覆盖；191 gateway测试及严格clippy通过。首轮游标WebSocket测试误把心跳当JSON而失败，已修测试帧处理并保留失败日志。
- 仅部署server7f0a5dd，PID46850；原pending及waiting checkpoint/browser session哈希保持。桌面/Android未重装。真实流式复验继续，S0仍唯一整阶段通过。远端GitHub443连接超时，本地main已合入；不得将本地合并说成已推送。
- 7f0a5dd真实模型只读既有marker后输出80行验收建议（不是执行这些建议）：41个message.delta、52个trace.delta、8个trace.item，实际桌面截图显示正文11行→60行→完整80行，实时轨迹自动追加到aseq72 run.end。关闭重开显示回放及全文，history.live=false。证据production-7f0a5dd-real-stream.json及实际查看的a/b/completed/replay-open截图。
- 关闭实时面板发现production trace.unsubscribe漏接，错误落入业务RPC。fe360a2修复连接层退订与参数校验，192 gateway测试和严格clippy通过；仅部署server，PID49651，旧pending/checkpoint/browser哈希保持。正式WebSocket实测8订阅、第9个conflict、退订释放后可补第8个、全部释放通过。原生等待任务重连仍显示实时，面板返回公告无退订错误；“收起”按钮本轮未观察到可靠关闭，使用面板返回键，单独按钮行为不计通过。
- GitHub连接恢复，main已推至fe360a2；最新pkg已审计为干净源码和一致daemon/sidecar，SHA e9d7f4d452286fd6b482be6c96379c578279c81a3e73620d7a3342b50869cf3a，未fresh安装。Android最新截图仍为System UI无响应；双端剩余场景及X登录仍有外部控制条件。S0仍唯一整阶段通过。
