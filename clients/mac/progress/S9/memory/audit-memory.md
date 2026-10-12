# macOS 用户记忆实现审计

## v1.1 §11：用户记忆可以编辑

- [x] 设置导航把「记忆」放在「通用」之后；桌面文案与 `design/03-macos.md` §6.7 的「设置 → 记忆」一致。证据：`App.tsx` 的 `SettingsPanel.sections`；`i18n.ts` 的 `settings.section.memory`。
- [x] 页面使用 `memory.user.list/create/update/delete`，提供按正文搜索、高亮、原地编辑、500 字限制、删除后 5 秒撤销、1500 字配额和冲突对比。证据：`MemoryPage.tsx`；`App.tsx` 的 `loadMemory` 与 `memoryRequest` / mutation callbacks。
- [x] 服务端事件更新记忆列表；应用切换 Host 时按 Host scope 重新加载，避免混用档案和条目。证据：`App.tsx` 的 `memory-protocol-event` 订阅与 `memoryDataScope`。
- [x] 只呈现用户记忆入口；Bot 与项目记忆不在该页面展示。记忆导航此前已存在，本节只把导航标题从「用户记忆」对齐为「记忆」。

