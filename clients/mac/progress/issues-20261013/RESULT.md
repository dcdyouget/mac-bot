# macOS 回归验证（2026-10-13）

## 版本与环境

- HEAD：`0a80911` (`desktop: fix sidebar and settings navigation`)
- 本轮实际打开的客户端：`clients/mac/app/src-tauri/target/debug/bundle/macos/Mac Bot.app/Contents/MacOS/macbot-desktop`
- 二进制 mtime：`2026-10-13 21:53:02 CST`；SHA-256：`49038ebd2501761c6a954418e03c00ce5a94b605857f5e2bed20246afced0455`
- 当前客户端 PID：`61099`；正式服务端 LaunchAgent PID：`30286`
- `http://127.0.0.1:7788/api/v1/health` 返回 `{"ok":true,"protocol":1,"setup_required":false,"version":"0.1.0"}`。
- 服务端修复部署后的 HEAD：`28dbd06`；该轮重用上述最新 debug bundle（SHA-256 未变化）。

## 自动化检查

- 定向前端测试：18/18 通过：`node --test src/components/messageActions.model.test.mjs src/navigationUi.test.mjs src/components/settingsCopyKeys.test.mjs src/components/aboutCopyKeys.test.mjs src/components/contextPanelWindow.model.test.mjs`
- `npm run build`：通过。
- `npm run tauri -- build --debug --no-bundle`：通过。
- `npm run tauri -- build --debug`：生成了本轮启动的 `.app`，之后 DMG 打包步骤失败；当前 debug app 本身可启动并连接正式服务端。

## 13 项复核

| # | 结果 | 证据/说明 |
|---|---|---|
| 1 流式消息预览 | 通过 | 最后回归在当前 debug bundle 上观察到等待气泡只显示三点动画，无“不支持预览”占位；最终正常渲染回复“收到”。等待态由 CUA 截图确认，但落盘截图触发时已完成；没有伪称等待态截图已保存。 |
| 2 消息悬浮三按钮 | 通过 | 当前客户端静止及指针落在消息上均未显示消息气泡旁的三按钮浮框。 |
| 3 右键菜单 | 通过 | 右键菜单仅有表情回应、回复、引用，无消息 ID、复制项。见 `right-click.png`。 |
| 4 侧栏按钮/展开方向 | 通过 | 聊天标题只有一个“群信息”侧栏按钮（另有搜索按钮）；展开侧栏后窗口宽度从 2784 增至 3344 像素，聊天区域宽度基本保持，面板向窗口右侧扩展。见 `sidebar-expanded.png`。 |
| 5 设置返回主 Bot | 通过 | 设置中点击左侧“主会话”后设置关闭并回到主 Bot 聊天。 |
| 6 设置内技能入口 | 通过 | 设置分类为通用、记忆、模型与服务商、运行时、工具与审批、电脑与浏览器、主机、About；没有技能分类。 |
| 7 主 Bot 齿轮 | 通过 | 主 Bot 会话行没有旁置齿轮按钮。 |
| 8 模型页空图标 | 通过 | 重试按钮显示刷新图标和文案；相邻新增服务商按钮显示“+”图标。见 `models-settings.png`。 |
| 9 运行时空列 | 通过 | 运行时只显示有标签的“主 Bot”和“记忆整理 / 压缩”模型选项，无空字段列。见 `runtime-settings.png`。 |
| 10 电脑页空权限行 | 通过 | 屏幕录制、辅助功能、完全磁盘访问、Chrome 自动化均有标签及状态。见 `computer-settings.png`。 |
| 11 技能页切换叠影 | 通过 | 技能页稳定显示；从技能页点击主会话后切回聊天，没有叠影。 |
| 12 Command+Enter | 通过 | 最后回归输入“请只回复：收到”并按 Command+Enter，消息成功发出并收到正常回复。 |
| 13 服务端意外停止 | 未复现 | 本轮服务端 PID `30286` 持续存活，7788 正常监听且健康检查通过；短时回归不能证明长期稳定性。 |

## 真实 API 请求的服务端错误

发现旧 bundle 后没有用它判定界面修复。关闭旧客户端前发出的唯一一条真实请求来自旧 bundle，但连接的是正式 7788 服务（LaunchAgent PID `30286`，配置页显示 MiniMax CN / `https://api.minimax.cn/anthropic` / `MiniMax-M3.1-Flash-Preview`）。服务端返回：

> `运行失败：execution sink: memory extraction failed: memory operation is not allowed: maintenance extraction returned an out-of-scope memory target`

这条消息随后由当前 debug 客户端从正式服务端加载；截图 `chat-current.png` 保留了结果。随后服务端修复部署到 LaunchAgent PID `2220`，最后一轮使用当前 debug bundle 发送唯一一条短请求，通过 Command+Enter 成功得到 MiniMax 回复“收到”。等待态三点和完成态分别见 `waiting-stream-final.png` 与 `completed-stream-final.png`。因此早先的记忆提取错误仍作为历史证据保留，但最后的真实 API 回归已成功。

## 截图

- `chat-current.png`：当前 debug app 的主会话及真实服务端错误
- `right-click.png`：右键菜单
- `sidebar-expanded.png`：侧栏展开及窗口扩展方向
- `runtime-settings.png`：运行时设置
- `models-settings.png`：模型与服务商按钮图标
- `computer-settings.png`：电脑权限文案
- `completed-stream-final.png`：最后回归的正常最终回复
- `completed-stream-capture-race.png`：预定等待态截图实际触发较晚，内容仍是完成态；仅作时间竞态记录，不作为等待态证据

仓库的 `screenshot-window.sh` 在本轮因写入临时截图 `.development-mock.png` 失败；最终证据由 `screencapture` 按当前 app 窗口 ID 直接保存。

最后一轮等待时，CUA 实时截图明确显示 assistant 气泡中的三点；真实回复很快到达。延迟落盘截图与最终截图均显示完成态，所以本目录没有等待态 PNG 文件。最终态 PNG 为 `completed-stream-final.png`。
