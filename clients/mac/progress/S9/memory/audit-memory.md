# macOS 用户记忆实现审计

## v1.1 §11：用户记忆可以编辑

- [x] 设置导航把「记忆」放在「通用」之后；桌面文案与 `design/03-macos.md` §6.7 的「设置 → 记忆」一致。证据：`App.tsx` 的 `SettingsPanel.sections`；`i18n.ts` 的 `settings.section.memory`。
- [x] 页面使用 `memory.user.list/create/update/delete`，提供按正文搜索、高亮、原地编辑、500 字限制、删除后 5 秒撤销、1500 字配额和冲突对比。证据：`MemoryPage.tsx`；`App.tsx` 的 `loadMemory` 与 `memoryRequest` / mutation callbacks。
- [x] 服务端事件更新记忆列表；应用切换 Host 时按 Host scope 重新加载，避免混用档案和条目。证据：`App.tsx` 的 `memory-protocol-event` 订阅与 `memoryDataScope`。
- [x] 只呈现用户记忆入口；Bot 与项目记忆不在该页面展示。记忆导航此前已存在，本节只把导航标题从「用户记忆」对齐为「记忆」。

## v1.1 §12：用户档案与记忆整理

- [x] 档案卡片位于说明和用量之间；无档案且正在加载时显示骨架。称呼 / 身份为单行输入，空字符串转 `null`；语言下拉首项也是 `null`，包含 `zh-CN`、`en-US`、`ja-JP`、`ko-KR`；时区只读。失焦、回车或语言变更会提交仅含目标字段和最新 `expected_version` 的更新。
- [x] 保存按字段排队串行执行；响应只回填仍等于已提交值的字段，保留等待期间继续输入的草稿。回归覆盖保存中同字段改写和另一字段输入；profile 冲突 warning 位于档案卡片上方，点击文案会调用页面真实 `onReload` 并用服务端档案重置草稿。
- [x] system 来源显示 16px sparkles、「记忆整理 根据」、会话标题链接和固定 `MM/dd HH:mm` 时间；user 显示「你手动添加」；旧 bot 来源显示 16px 圆头像和 Bot 名称，并保留可用的会话链接。
- [x] 补全指定 `memory.profile.*`、`memory.source.*`、`memory.desc`、`memory.quota.warning_text` 文案 key；配额计算在达到上限时自动钳制为 100%，添加 / 保存失败提示位于工具行下方。
- [x] 所有记忆页界面文案使用直接 typed `t()` key，无 `copy()` 或 JSX 文案 fallback；条目正文由可访问按钮触发编辑，来源链接作为独立控件；profile 保存期间仍可继续编辑并进入串行队列；Enter 在 IME 组合期间不保存。骨架与成功勾选动画遵循 `prefers-reduced-motion`。
- [x] 满配额 danger 提示按服务端用量自动显示并保持在添加按钮下方；开始添加或取消编辑不会清掉提示，`keepMine` 成功后同步 quota。
- [x] 验证：`pnpm --config.ignore-scripts=true tsc --noEmit` 通过；`pnpm --config.ignore-scripts=true test:memory` 通过，6/6，覆盖串行 profile 保存、草稿保留、满额判断和 IME Enter 保护。默认 `pnpm tsc --noEmit` 曾触发 pnpm 安装并被仓库的 ignored build script 设置拦截，未修改依赖清单；切到 ignore-scripts 后同一 TypeScript 检查通过。
- [ ] 本 subagent 未启动 UI 或生成截图；最终 mock 交互与五张截图由独立 QA 负责。

