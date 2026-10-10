# S5 fresh 证据索引

状态：**进行中，S5 未通过**。本索引只整理现有 JSON、截图和失败边界，不改写 Bot 原始产物；截图是否实际查看以 root 验收记录为准。

## 安装、部署与版本边界

- 3d6b289 native pkg 安装、管理页密码、fresh home 和旧业务 hash：[`fresh-install-success-audit.json`](fresh-install-success-audit.json)、[`fresh-install-journal.json`](fresh-install-journal.json)、[`fresh-admin-configured.png`](fresh-admin-configured.png)、[`fresh-setup-observation-audit.json`](fresh-setup-observation-audit.json)。安装前 `setup_required=true` 未观测，不能回填为已通过。
- 最终 server `a9527fa4a0f52b5ff6e47c380ac0819e8292127d` 的 user-App payload 已部署并健康；系统 native Installer receipt 仍为 3d6b289。clean archive 候选 pkg 的 `deployment.performed=false`、`installed=false`、`service_touched=false` 保持不变，不能把准备包当成当前 native 服务：[`server-pkg-a9527fa-candidate.json`](server-pkg-a9527fa-candidate.json)、[`fresh-server-a9527fa-update.json`](fresh-server-a9527fa-update.json)。启动恢复的早期 health 快照与最终 denied job 收敛见 [`fresh-recovery-denied-audit.json`](fresh-recovery-denied-audit.json)。
- 23c34ba 原始 pkg candidate：[`server-pkg-23c34ba-candidate.json`](server-pkg-23c34ba-candidate.json)。其中 `deployment.performed=false`、`installed=false` 保持不变，不能把 candidate 当作 native pkg 已安装。
- 23c34ba earlier user-App payload update：[`fresh-server-23c34ba-update.json`](fresh-server-23c34ba-update.json)、[`fresh-memory-cursor-postdeploy.json`](fresh-memory-cursor-postdeploy.json)；1d56a47 current user-App payload update：[`fresh-server-1d56a47-update.json`](fresh-server-1d56a47-update.json)。两者均是用户服务 App 载荷更新，系统 native Installer receipt 仍为 3d6b289；1d56a47 的 candidate [`server-pkg-1d56a47-candidate.json`](server-pkg-1d56a47-candidate.json) 中 `deployment.performed=false`、`installed=false`、`service_touched=false`，不能当作 native pkg 已安装。
- 桌面当前为已部署的 bdc17cdf 诊断版；首帧已 applied 但未触发重绘，仍在调查：[`desktop-dmg-bdc17cdf-candidate.json`](desktop-dmg-bdc17cdf-candidate.json)、[`fresh-desktop-bdc17cdf-update.json`](fresh-desktop-bdc17cdf-update.json)、[`fresh-s4-bdc-first-frame-stalled.png`](fresh-s4-bdc-first-frame-stalled.png)。不宣称 ACK stall 已修复或 S4/S5 通过；既有无帧和低清失败证据继续保留。
- Android 已安装并保留应用数据的 78bafff Release：[`fresh-android-78bafff-update.json`](fresh-android-78bafff-update.json)、[`fresh-android-postupdate-ui-timeout.json`](fresh-android-postupdate-ui-timeout.json)。当前连接尚未恢复；性能验收仍 pending，系统 UI ANR 与服务重连延迟保持为独立证据，不能计 fresh 通过。

## S1 单 Bot

- 工具 roundtrip：[`fresh-s1-tool-roundtrip.json`](fresh-s1-tool-roundtrip.json)、[`fresh-s1-desktop-trace.png`](fresh-s1-desktop-trace.png)、[`fresh-s1-android-trace.png`](fresh-s1-android-trace.png)。
- kill-9 同 run 恢复：[`fresh-recovery-kill.json`](fresh-recovery-kill.json)、[`fresh-recovery-desktop-done.png`](fresh-recovery-desktop-done.png)、[`fresh-recovery-android-resume.png`](fresh-recovery-android-resume.png)。首次未执行 kill 的失败边界保留在 [`fresh-recovery-first-not-killed.json`](fresh-recovery-first-not-killed.json) 和 [`fresh-recovery-desktop-running.png`](fresh-recovery-desktop-running.png)。
- 原生实时流式复验：[`fresh-s1-stream-frames.json`](fresh-s1-stream-frames.json) 记录桌面 cache 收到的流事件和 50 段全文；[`fresh-s1-postfix-desktop-latest-after-layout.png`](fresh-s1-postfix-desktop-latest-after-layout.png) 已实际查看，证明 after-layout 画面边界。早先 [`fresh-s1-native-stream-observation.json`](fresh-s1-native-stream-observation.json) 与 [`fresh-s1-native-stream-desktop-1.png`](fresh-s1-native-stream-desktop-1.png) 属于旧 scroll/capture 观察，不能解释为新 server 序号缺口；正文末尾的模型生成“完成”文字仍不替代观测结论。
- Android UI 阻塞证据：[`fresh-s1-native-stream-android-1.png`](fresh-s1-native-stream-android-1.png)、[`fresh-s1-postfix-android-stream-1.png`](fresh-s1-postfix-android-stream-1.png) 保留真实画面和重连边界；Android 当前 UI 阻塞，S1 仍未收口。旧桌面 postfix 图 [`fresh-s1-postfix-desktop-stream-1.png`](fresh-s1-postfix-desktop-stream-1.png) 仅作历史 scroll/capture 失败证据。

## S2 主 Bot、双群与 Tester

- 双群并行和同 assignment/run 插话：[`fresh-s2-busy-parallel-audit.json`](fresh-s2-busy-parallel-audit.json)。该文件明确只证明路由、同 run 应用和并发重叠，不证明 Tester 和最终 review。
- A 项目 Tester 已返回五项真实浏览器结果（wrong password、correct login、refresh、logout、375x800 viewport；`innerWidth=375`、`scrollWidth=375`），见 [`fresh-rpc/a-tester-047.json`](fresh-rpc/a-tester-047.json) 中的同项目 review 请求及其引用的 done/report ID。A 项目仍为 `active`，主 Bot review 已排队，不能写成 project done 或 S2 通过。
- B 项目仍在真实浏览器验收中；最新证据 [`fresh-rpc/b-review-056.json`](fresh-rpc/b-review-056.json)、[`fresh-rpc/b-review-055.json`](fresh-rpc/b-review-055.json) 仍是同项目 browser approval/等待链路，尚无完成报告结论，不能宣称 B 或 S2 通过。
- duplicate project 现场和停止/隔离边界：[`fresh-s2-collision-before.json`](fresh-s2-collision-before.json)、[`fresh-rpc/15-stop-fresh-duplicate-assignment.json`](fresh-rpc/15-stop-fresh-duplicate-assignment.json)、[`fresh-rpc/16-isolate-fresh-duplicate-home.json`](fresh-rpc/16-isolate-fresh-duplicate-home.json)、[`fresh-rpc/17-archive-fresh-duplicate.json`](fresh-rpc/17-archive-fresh-duplicate.json)。

## S3 技能、用量、记忆与搜索

- 技能 v2 保存：[`fresh-s3-native-update.json`](fresh-s3-native-update.json)、[`fresh-s3-desktop-skill-v2.png`](fresh-s3-desktop-skill-v2.png)、[`fresh-s3-android-skill-v2.png`](fresh-s3-android-skill-v2.png)。
- 停用/重新启用同步：[`fresh-s3-native-disable.json`](fresh-s3-native-disable.json)、[`fresh-s3-desktop-skill-created.png`](fresh-s3-desktop-skill-created.png)、[`fresh-s3-android-skill-disabled.png`](fresh-s3-android-skill-disabled.png)。这是局部技能范围证据，不代表完整 S3 CRUD、用量、记忆和搜索均已 fresh 复验。

## S4 浏览器画面、接管与低清边界

- 当前 FreshScreen 边界：桌面 bdc17cdf 诊断版已部署，但首帧 applied 后未触发重绘，仍在诊断，不能据此记 S4 通过。现场见 [`desktop-dmg-bdc17cdf-candidate.json`](desktop-dmg-bdc17cdf-candidate.json)、[`fresh-desktop-bdc17cdf-update.json`](fresh-desktop-bdc17cdf-update.json)、[`fresh-s4-bdc-first-frame-stalled.png`](fresh-s4-bdc-first-frame-stalled.png)。
- 旧版本原生输入提交与同 Bot readback：[`fresh-s4-native-takeover.json`](fresh-s4-native-takeover.json)、[`fresh-s4-desktop-frame-1.png`](fresh-s4-desktop-frame-1.png)、[`fresh-s4-desktop-input-before-submit-visible.png`](fresh-s4-desktop-input-before-submit-visible.png)、[`fresh-s4-desktop-input-submitted.png`](fresh-s4-desktop-input-submitted.png)。该部分是 ad2d915 历史证据，不覆盖当前无帧诊断，也不等于 S4 全阶段通过。
- 原生低清切换失败：[`fresh-s4-desktop-low-empty.png`](fresh-s4-desktop-low-empty.png)。画面为空，不能把低清 native switch 标为通过。
- 低清 transport-only 证据：[`fresh-s4-low-transport/manifest.json`](fresh-s4-low-transport/manifest.json)、[`fresh-s4-low-transport/frame-01-seq-1.jpg`](fresh-s4-low-transport/frame-01-seq-1.jpg)、[`fresh-s4-low-transport/frame-02-seq-2.jpg`](fresh-s4-low-transport/frame-02-seq-2.jpg)。manifest 明确为 `transport_only_pass`，不包含 native UI/input 成功结论。

## 当前最短剩余清单

S1 桌面 50 段全文和 after-layout 已有证据，剩余 Android UI 阻塞需解除并重新取得联合实时流式证据；S2 的 A Tester 五项真实结果已返回但项目仍 active、主 Bot review 尚未收口，B 仍在验收中；S3 需要补齐 fresh 双端剩余场景；S4 需继续排查 bdc 诊断版首帧 applied 后未重绘，并重新验证低清切换、通知、定时任务及浏览器联合场景；a9527fa user-App payload 已部署并健康，但 native receipt 仍为 3d6b289，不能据此宣称 S5 通过。完成后才可重新评估 S5。所有原始失败截图和 Bot 生成正文均保留，不以文字“完成”替代实际观测。
