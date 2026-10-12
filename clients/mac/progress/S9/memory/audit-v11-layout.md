## v1.1 §9 公告自由文本

- **已覆盖**：公告按自由 Markdown 保存为单一 `text` 字段，保存携带 `base_version`；原有编辑提示明确说明 Markdown 和 Bot 可读写。群公告详情现在复用 `ArtifactPage` 的安全 React Markdown 渲染器，不插入 HTML。
- **补齐**：版本冲突后重新请求 `project.get`，展示本地与服务端两版内容和版本号；用户可用最新版本，或以最新版本号重试保存自己的内容。若刷新失败但冲突错误含 `details.current`，仍展示冲突快照。
- **证据**：`clients/mac/app/src/App.tsx`（`saveAnnouncement`、冲突比较/恢复操作、MarkdownPreview）；`clients/mac/app/src/components/ArtifactPage.tsx`（共享 Markdown 渲染器导出）；`clients/mac/app/src/styles.css`（公告冲突比较样式）；`clients/mac/app/src/i18n.ts`（冲突操作文案 key）。
- **主审修正**：公告保存/冲突刷新锁定触发时的 `project_id`，Host 或当前群变化后丢弃异步响应；进入其他群时清空冲突，开始编辑/取消也重置。出现冲突后禁用普通保存和 ⌘↵，只能明确选择用服务端版本或以最新版本重试本地文本。
- **验收**：静态检查待整批提交前执行；尚未做 mock 界面 QA。

## v1.1 §10 页面与布局调整

- **栏宽与窄窗**：主会话侧栏基础宽 260px、会话区 `min-width: 480px`、右面板 280px；列表/详情页如技能、定时任务、设置和产物各自有 260px 列表导航。保留设计规定的窄窗行为：<1100px 右面板覆盖；<820px 侧栏隐藏为 260px 覆盖层，可用工具栏按钮或 ⌘\\ 打开，选会话/遮罩关闭；⌘⇧\\ 切换右面板。移除了 1200/1000/860 处把侧栏缩成 220/200/176px 的不一致规则，其他页面密度适配断点保留。
- **右面板/页面入口**：主 Bot 私聊展示本对话待处理项和跟进群；Bot 私聊展示审批、进行中任务和队列；群聊展示 Markdown 公告、成员、产物与工作详情。定时任务为侧栏独立页面，工作台、仪表盘、技能也有独立入口。Host 菜单支持切换、添加、管理。
- **提醒与工作台**：补上群、Bot、主 Bot 会话行的 pending approval 橙色提示、徽标和左侧警示条；工作台任务已有 [群聊] 跳转（统一使用 `follow.open_group` 文案）、详情、@它、停止和待看数。
- **产物与其他屏幕**：群产物显示作者和预览入口；`ArtifactPage` 提供预览/源文件/历史；Bot 详情/设置、主 Bot 设置、群聊创建、拖入文件、会话完成/空状态、设置分页、常规窄窗页面密度和 dashboard 下钻在现有组件中实现。
- **证据**：`clients/mac/app/src/App.tsx`（侧栏 Host 菜单/页面入口/右面板/approval row、窄窗行为）；`clients/mac/app/src/styles.css`（栏宽、覆盖面板与侧栏、approval 标识）；`clients/mac/app/src/components/WorkbenchPage.tsx`（任务群聊入口）；`clients/mac/app/src/components/{RoutinesPage,SkillsPage,ArtifactPage,DashboardPage,BotPanel,GroupSettingsPage}.tsx`。
- **验收**：`node node_modules/typescript/bin/tsc --noEmit` 通过；尚未做 mock 界面 QA。
- **限制**：Host 列表没有独立的离线持续时间数据，当前离线态显示状态点；审批提醒摘要取每个会话第一条 pending approval。
- **主审修正**：approval 按 `assignment_id → project_id → chat_id` 同时映射到群行，徽标显示待审批数量；窄窗的导航按钮改为 app-shell 全局入口，工作台/仪表盘/定时任务等页面也能打开侧栏，Esc 关闭侧栏覆盖层。
