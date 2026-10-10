# S5 fresh 证据索引

状态：**进行中，S5 未通过**。本索引只整理现有 JSON、截图和失败边界，不改写 Bot 原始产物；截图是否实际查看以 root 验收记录为准。

## 安装、部署与版本边界

- 3d6b289 native pkg 安装、管理页密码、fresh home 和旧业务 hash：[`fresh-install-success-audit.json`](fresh-install-success-audit.json)、[`fresh-install-journal.json`](fresh-install-journal.json)、[`fresh-admin-configured.png`](fresh-admin-configured.png)、[`fresh-setup-observation-audit.json`](fresh-setup-observation-audit.json)。安装前 `setup_required=true` 未观测，不能回填为已通过。
- b9d0cda 健康基线和 03afbd2 旧载荷记录保留；当前 server `cf6b5bbc40def3a6015a41998c94905cbb35965f` 已作为 user-App payload 部署，PID `84786`。cf6b5bb 只读复核确认 seq15 orphan 恢复为唯一 stable run，`memory_search` 返回 1 entry，seq16 回复成功，未重发 RPC；见 [`fresh-server-cf6b5bb-update.json`](fresh-server-cf6b5bb-update.json)。系统 native Installer receipt 仍为 3d6b289。
- 23c34ba 原始 pkg candidate：[`server-pkg-23c34ba-candidate.json`](server-pkg-23c34ba-candidate.json)。其中 `deployment.performed=false`、`installed=false` 保持不变，不能把 candidate 当作 native pkg 已安装。
- 23c34ba earlier user-App payload update：[`fresh-server-23c34ba-update.json`](fresh-server-23c34ba-update.json)、[`fresh-memory-cursor-postdeploy.json`](fresh-memory-cursor-postdeploy.json)；1d56a47 current user-App payload update：[`fresh-server-1d56a47-update.json`](fresh-server-1d56a47-update.json)。两者均是用户服务 App 载荷更新，系统 native Installer receipt 仍为 3d6b289；1d56a47 的 candidate [`server-pkg-1d56a47-candidate.json`](server-pkg-1d56a47-candidate.json) 中 `deployment.performed=false`、`installed=false`、`service_touched=false`，不能当作 native pkg 已安装。
- 桌面当前为已部署的 bdc17cdf 诊断版；首帧已 applied 但未触发重绘，仍在调查：[`desktop-dmg-bdc17cdf-candidate.json`](desktop-dmg-bdc17cdf-candidate.json)、[`fresh-desktop-bdc17cdf-update.json`](fresh-desktop-bdc17cdf-update.json)、[`fresh-s4-bdc-first-frame-stalled.png`](fresh-s4-bdc-first-frame-stalled.png)。不宣称 ACK stall 已修复或 S4/S5 通过；既有无帧和低清失败证据继续保留。
- Android 9f9a1cf 同签名 Release 已覆盖安装并进入 B 待验收；B review 画面出现 request timeout（Android 30 秒），不能记连接稳定通过；此前 953c53b 真实输入提交为 green，但 native 交还仍未闭环，不能计完整 S4/S5。9f9a1cf 构建/签名记录见 [`fresh-android-9f9a1cf-update.json`](fresh-android-9f9a1cf-update.json)；此前 953 的连续画面、控制延迟和 release pending 证据仍保留在 [`fresh-s4-android-screen-takeover.json`](fresh-s4-android-screen-takeover.json)、[`fresh-s4-android-control-delay-audit.json`](fresh-s4-android-control-delay-audit.json) 和 [`fresh-s4-android-release-pending.png`](fresh-s4-android-release-pending.png)。

## S1 单 Bot

- 工具 roundtrip：[`fresh-s1-tool-roundtrip.json`](fresh-s1-tool-roundtrip.json)、[`fresh-s1-desktop-trace.png`](fresh-s1-desktop-trace.png)、[`fresh-s1-android-trace.png`](fresh-s1-android-trace.png)。
- kill-9 同 run 恢复：[`fresh-recovery-kill.json`](fresh-recovery-kill.json)、[`fresh-recovery-desktop-done.png`](fresh-recovery-desktop-done.png)、[`fresh-recovery-android-resume.png`](fresh-recovery-android-resume.png)。首次未执行 kill 的失败边界保留在 [`fresh-recovery-first-not-killed.json`](fresh-recovery-first-not-killed.json) 和 [`fresh-recovery-desktop-running.png`](fresh-recovery-desktop-running.png)。
- 原生实时流式复验：[`fresh-s1-stream-frames.json`](fresh-s1-stream-frames.json) 记录桌面 cache 收到的流事件和 50 段全文；[`fresh-s1-postfix-desktop-latest-after-layout.png`](fresh-s1-postfix-desktop-latest-after-layout.png) 已实际查看，证明 after-layout 画面边界。早先 [`fresh-s1-native-stream-observation.json`](fresh-s1-native-stream-observation.json) 与 [`fresh-s1-native-stream-desktop-1.png`](fresh-s1-native-stream-desktop-1.png) 属于旧 scroll/capture 观察，不能解释为新 server 序号缺口；正文末尾的模型生成“完成”文字仍不替代观测结论。
- Android UI 阻塞证据：[`fresh-s1-native-stream-android-1.png`](fresh-s1-native-stream-android-1.png)、[`fresh-s1-postfix-android-stream-1.png`](fresh-s1-postfix-android-stream-1.png) 保留历史真实画面和重连边界；953c53b 当前 dashboard 已 fresh ready，但 S1 联合实时流式/输入与性能仍未收口。旧桌面 postfix 图 [`fresh-s1-postfix-desktop-stream-1.png`](fresh-s1-postfix-desktop-stream-1.png) 仅作历史 scroll/capture 失败证据。

## S2 主 Bot、双群与 Tester

- 双群并行和同 assignment/run 插话：[`fresh-s2-busy-parallel-audit.json`](fresh-s2-busy-parallel-audit.json)。该文件明确只证明路由、同 run 应用和并发重叠，不证明 Tester 和最终 review。
- A 项目 Tester 已返回五项真实浏览器结果（wrong password、correct login、refresh、logout、375x800 viewport；`innerWidth=375`、`scrollWidth=375`），见 [`fresh-rpc/a-tester-047.json`](fresh-rpc/a-tester-047.json)；A 双端 review 截图 [`fresh-s2-a-android-review.png`](fresh-s2-a-android-review.png)、[`fresh-s2-a-desktop-review.png`](fresh-s2-a-desktop-review.png) 已由 root 实际查看。A 项目仍待验收，尚未 `confirm_done`，不能写成 project done 或 S2 通过。
- B 已在同一 assignment/run 以修正 slug 成功发起 `request_review`；[`fresh-s2-b-desktop-review.png`](fresh-s2-b-desktop-review.png) 与 [`fresh-s2-b-android-9f9a1cf-review.png`](fresh-s2-b-android-9f9a1cf-review.png) 已实际查看，但两端画面同时出现 request timeout（Android 30 秒），不能记连接稳定通过。旧 report-only 068/069 的退出项 `PARTIAL` 保留（旧报告未合并后续退出后 snapshot），见 [`fresh-rpc/b-review-068.json`](fresh-rpc/b-review-068.json)、[`fresh-rpc/b-review-069.json`](fresh-rpc/b-review-069.json)。B 仍是 review 待验收，不能宣称 S2 通过。
- duplicate project 现场和停止/隔离边界：[`fresh-s2-collision-before.json`](fresh-s2-collision-before.json)、[`fresh-rpc/15-stop-fresh-duplicate-assignment.json`](fresh-rpc/15-stop-fresh-duplicate-assignment.json)、[`fresh-rpc/16-isolate-fresh-duplicate-home.json`](fresh-rpc/16-isolate-fresh-duplicate-home.json)、[`fresh-rpc/17-archive-fresh-duplicate.json`](fresh-rpc/17-archive-fresh-duplicate.json)。

## S3 技能、用量、记忆与搜索

- 技能 v2 保存：[`fresh-s3-native-update.json`](fresh-s3-native-update.json)、[`fresh-s3-desktop-skill-v2.png`](fresh-s3-desktop-skill-v2.png)、[`fresh-s3-android-skill-v2.png`](fresh-s3-android-skill-v2.png)。
- 停用/重新启用同步：[`fresh-s3-native-disable.json`](fresh-s3-native-disable.json)、[`fresh-s3-desktop-skill-created.png`](fresh-s3-desktop-skill-created.png)、[`fresh-s3-android-skill-disabled.png`](fresh-s3-android-skill-disabled.png)。这是局部技能范围证据，不代表完整 S3 CRUD、用量、记忆和搜索均已 fresh 复验；cf6b5bb 已补齐 seq15 orphan 唯一 run、memory_search 1 entry、seq16 回复的服务端证据，跨 run 条目截图 [`fresh-s3-desktop-memory-recovered.png`](fresh-s3-desktop-memory-recovered.png) 与 [`fresh-s3-android-memory-recovered.png`](fresh-s3-android-memory-recovered.png) 已实际看到；Android 同时重连并出现 30 秒超时，不计整 S3 通过。

## S4 浏览器画面、接管与低清边界

- 当前 FreshScreen 边界：桌面 bdc17cdf AX 可读且截图可取，routine 详情已实际查看，但坐标点击返回 `noWindowsAvailable`，证据见 [`fresh-s4-desktop-input-tool-block.json`](fresh-s4-desktop-input-tool-block.json)；这不是锁屏证据，持续画面仍未闭环。Android 953c53b 的真实输入提交为 green（9f9a1cf 尚未重测此项），但 native 交还仍未闭环。目标 routine 实际 schedule 成功但晚 `94.255847` 秒；目标通知曾被 native AX 观察到，后被旧 A 通知洪水挤出，截图未捕获目标，打开动作未验收，routine 已禁用，见 [`fresh-s4-scheduled-conclusion.json`](fresh-s4-scheduled-conclusion.json)、[`fresh-s4-notification-after-scheduled.json`](fresh-s4-notification-after-scheduled.json)。不能据此记 S4 通过。
- 旧版本原生输入提交与同 Bot readback：[`fresh-s4-native-takeover.json`](fresh-s4-native-takeover.json)、[`fresh-s4-desktop-frame-1.png`](fresh-s4-desktop-frame-1.png)、[`fresh-s4-desktop-input-before-submit-visible.png`](fresh-s4-desktop-input-before-submit-visible.png)、[`fresh-s4-desktop-input-submitted.png`](fresh-s4-desktop-input-submitted.png)。该部分是 ad2d915 历史证据，不覆盖当前无帧诊断，也不等于 S4 全阶段通过。
- 原生低清切换失败：[`fresh-s4-desktop-low-empty.png`](fresh-s4-desktop-low-empty.png)。画面为空，不能把低清 native switch 标为通过。
- 低清 transport-only 证据：[`fresh-s4-low-transport/manifest.json`](fresh-s4-low-transport/manifest.json)、[`fresh-s4-low-transport/frame-01-seq-1.jpg`](fresh-s4-low-transport/frame-01-seq-1.jpg)、[`fresh-s4-low-transport/frame-02-seq-2.jpg`](fresh-s4-low-transport/frame-02-seq-2.jpg)。manifest 明确为 `transport_only_pass`，不包含 native UI/input 成功结论。

## 当前最短剩余清单

S1 桌面 50 段全文和 after-layout 已有证据；S2 A 双端 review 截图已由 root 实际查看但尚未 confirm_done，B 已在同 run 以修正 slug 发起 request_review，旧报告退出项 PARTIAL 保留；S3 seq15 orphan 唯一 run、memory_search 1 entry、seq16 回复已由 cf6b5bb 复核，但双端 S3 仍未收口；S4 Android 输入提交 green 但 native 交还未闭环，桌面 routine 详情已看而持续画面缺失，目标通知实际出现后被旧 A 洪水挤出且未验收打开。仍需完成桌面持续画面、Android native release、X、通知打开及联合场景；cf6b5bb 已部署但 full S5 不能通过。所有原始失败截图和 Bot 生成正文均保留，不以文字“完成”替代实际观测。
