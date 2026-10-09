# client-mac 阶段证据

S0 正式两端会话列表验收见 `../../../docs/progress/S0/main-eb088fa-desktop-sessions.png` 与根目录 COORDINATION.md。

S1–S5 的窗口截图来自隔离 `MacBotQA`（独立 bundle、file 后端、数据根和 7790 mock），属于客户端开发验收。来源 SHA、窗口 PID、服务来源、图片摘要和断言见 [verification.json](verification.json)。mock 与真实服务的阶段联合验收分开记录。

| 阶段 | 原生证据 |
| --- | --- |
| S1 | [重连历史恢复](S1/main-history-restored.png)、[私聊与轨迹](S1/private-trace.png) |
| S2 | [工作台](S2/workbench.png)、[新建群](S2/new-group.png)、[公告](S2/group-announcement.png) |
| S3 | [仪表盘](S3/dashboard.png)、[小时趋势](S3/dashboard-hourly.png)、[用量明细](S3/dashboard-details.png)、[技能保存重读](S3/skill-readback.png) |
| S4 | [接管画面](S4/computer-takeover.png)、[定时任务试运行](S4/routine-run.png) |
| S5 | [关闭后标准重开](S5/window-reopened.png)、[状态合并性能](S5/state-bench.json) |

核心 33 + 桌面 32 项测试与严格 Clippy 通过；实际 DMG 校验及仓库外临时目录替换测试通过。性能记录没有测量 GPUI 帧率；10 万条状态基准不能作为真实 UI 延迟承诺。公开更新源、正式签名/公证和真实安装版更新重启仍待分发配置与集成验收。
