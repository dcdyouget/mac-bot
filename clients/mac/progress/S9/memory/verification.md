# 桌面记忆与设计批次验收记录

最终验收标识（`clients/mac/app` 源码子树 hash）：`27c0964619876f1537f5fe9373ef577d45fb81c9`。

类型检查、Node 测试与 Tauri build 输出见同目录 `verification.log`。`PNPM_CONFIG_IGNORE_SCRIPTS=true pnpm tsc --noEmit` exit 0；根 `tsconfig.json` 的 `files` 为空，且 `references` 指向真实的 `tsconfig.app.json` 与 `tsconfig.node.json` 项目，因此另执行 `PNPM_CONFIG_IGNORE_SCRIPTS=true pnpm exec tsc -p tsconfig.app.json --noEmit` 验证 app 源码，exit 0。全量现有前端 Node 测试 37/37 通过。`PNPM_CONFIG_IGNORE_SCRIPTS=true pnpm tauri build --debug --bundles app` 成功，debug bundle 位于 `clients/mac/app/src-tauri/target/debug/bundle/macos/Mac Bot.app`。构建只有 Vite chunk 大于 500KB 的提示，没有失败。实现 agent 另报告 `message_file_*` 三项测试、`cargo check --all-targets`、`cargo fmt --check` 均通过；验证 agent未重复执行 native 检查。

隔离 mock 在 `localhost:7819` 提供真实 API，由已有 `/Users/gongshaojie/Project/mac-bot/server/target/debug/macbotd` 运行，`MACBOT_HOME` 为 `clients/mac/progress/S9/memory/.q/server`；启动参数 `--mock --port 7819 --password dev`，RPC 用 Bearer `dev`。只读复现 `GET /api/v1/health` 与 `memory.profile.get`、`memory.user.list`：档案为 `name/language/identity = null`、`onboarded=false`、版本 1；seed 列表含两条 `source.by.kind=system`（有真实 chat/message ID）和一条 `user`，配额 46/1500。后续写入只在其隔离副本 `localhost:7820` 与 `localhost:7821` 进行；副本和进程已清理，未改服务端源码。

真实 RPC 状态证据：档案版本 1 更新称呼成功返回版本 2；再以 `expected_version=1` 更新收到 `code=conflict` 且 `details.current.version=2`；用版本 2 传 `name:null` 成功清空并升到版本 3。随后一次更新 `{name:"小明", language:"zh-CN", identity:"产品经理"}` 成功到版本 4，`memory.profile.get` 仍返回 `onboarded=false`，因此无法用客户端编辑 API 造出协议意义上的已完成 onboarding，不能把第 05 张伪称为 onboarded。记忆条目 `mem_mock_1` 从版本 1 更新到 2，陈旧版本更新返回 conflict/current，版本 2 删除后配额从 37 降到 31。另一隔离副本按 500、500、454 字新建后 quota 依次为 546、1046、1500；再添加 1 字返回 `quota_full`。这只证明 mock API；不代表 UI 已经跑通。

已从源码审查 profile 保存队列、跨字段草稿保留、null 清空、Enter/blur 去重和冲突 reload 路径；来源链接会拉取分页历史并滚动、高亮指定消息。公告冲突比较/重载和同 Host/项目范围也做了静态核对。以上仍需要可见桌面上的真实交互验收。

此前原生截屏显示 macOS 登录锁屏，CUA 绑定 debug.app 路径返回 `cgWindowNotFound`。按主 agent 指示，截图移交后续独立 UI 验证；本次未生成或伪造产品截图。预期文件名：

- `clients/mac/progress/S9/memory/01-default.png`
- `clients/mac/progress/S9/memory/02-profile-edit.png`
- `clients/mac/progress/S9/memory/03-conflict.png`
- `clients/mac/progress/S9/memory/04-quota-full.png`
- `clients/mac/progress/S9/memory/05-onboarded.png`
