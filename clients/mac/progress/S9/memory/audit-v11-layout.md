## v1.1 §9 公告自由文本

- **已覆盖**：公告按自由 Markdown 保存为单一 `text` 字段，保存携带 `base_version`；原有编辑提示明确说明 Markdown 和 Bot 可读写。群公告详情现在复用 `ArtifactPage` 的安全 React Markdown 渲染器，不插入 HTML。
- **补齐**：版本冲突后重新请求 `project.get`，展示本地与服务端两版内容和版本号；用户可用最新版本，或以最新版本号重试保存自己的内容。若刷新失败但冲突错误含 `details.current`，仍展示冲突快照。
- **证据**：`clients/mac/app/src/App.tsx`（`saveAnnouncement`、冲突比较/恢复操作、MarkdownPreview）；`clients/mac/app/src/components/ArtifactPage.tsx`（共享 Markdown 渲染器导出）；`clients/mac/app/src/styles.css`（公告冲突比较样式）；`clients/mac/app/src/i18n.ts`（冲突操作文案 key）。
- **主审修正**：公告保存/冲突刷新锁定触发时的 `project_id`，Host 或当前群变化后丢弃异步响应；进入其他群时清空冲突，开始编辑/取消也重置。出现冲突后禁用普通保存和 ⌘↵，只能明确选择用服务端版本或以最新版本重试本地文本。
- **验收**：静态检查待整批提交前执行；尚未做 mock 界面 QA。
