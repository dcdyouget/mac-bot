# 剩余原生验收（2026-10-10）

仅 S0 整阶段通过。此表保留已有真实证据，不要求重建项目或重跑已完成 Tester。

| 阶段 | 可沿用证据 | 最短剩余 |
| --- | --- | --- |
| S1 | 桌面 `S1/production-7f0a5dd-real-stream.json` 的流式/实时轨迹/回放；Android `S1/production-09622d7-android-recovery.json` 同 run kill-9 恢复、Bash未重放 | 当前桌面与Android联合场景核对，补桌面实时订阅到重启后同run恢复/回放；不得用旧 `native_live_trace_passed=false` 记录充当成功 |
| S2 | 原两个项目真实 Tester 报告、112.033秒并行、同assignment插话、Android两张完整待验收卡；`S2/production-14a3b18-artifact-dedup-upgrade-after.json` | 桌面查看去重卡及最终矩阵核对后再确认完成；不提前confirm_done，不处理旧siblings或未知审批 |
| S3 | 两端固定UTC日摘要/明细一致；桌面维度范围修复；Android技能CRUD、真实Bot草稿/通知审批、原始无marker问句的记忆回忆 | 桌面草稿及记忆展示、原草稿发布与范围检查；修复并复测搜索内部记忆误分类；不再写一份偏好或重复模型请求 |
| S4 | 桌面已登录X读取/摘要；已有本地连续画面与输入局部证据；真实定时通知、打开、草稿通知“允许一次” | Android接管完整场景与两端画面/输入联合核对；通知容量仍仅有回归测试，不能宣称设备边界通过 |
| S5 | 最新桌面Release App/DMG签名/镜像校验，Android同签名更新保数据 | 最终server pkg重建审计；按fresh-install-plan可逆备份→卸载→pkg→管理页设密码→两端连接→完整场景 |

搜索误分类现已修复至 server `aa2c99a`，8项回归及Android同关键词/群筛选实测通过，失败证据保留；表中该修复项已完成，桌面展示仍待补。

桌面已解锁，正常重开客户端后CUA坐标输入和截图恢复；此前锁屏失败保留，不判客户端冻结。S5管理页新密码按CUA规则必须由用户输入、确认并提交，需先准备好页面才交接。

最新补充：桌面两张去重待验收卡已实看；真实草稿已由桌面发布，Android仅停用指定Bot且桌面复核。搜索修复双端已实看。桌面范围标题修复包待安装复核。两个项目还有5个历史待审批assignment，现有confirm_done会连带取消它们，暂不执行。S1联合恢复已进入原run的精确Bash审批等待，尚未kill；重启前正在核对一个旧running job是否为历史孤儿。

通知容量：现有实现最多40条应用通知，已有50→40的单元/Robolectric回归。当前模拟器有来源不明的旧通知；不以清除旧通知或注入假通知来伪造真实provider验收。若做容量设备专项，需隔离且可恢复的设备环境，并明确区分容量专项与真实业务链路。

主仓库COORDINATION及权威文档存在其他会话未提交改动，继续保留；本轮事实暂记progress，不混入根文档提交。
