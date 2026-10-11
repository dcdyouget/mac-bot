# Mac Bot 视觉设计稿 v1.1

> **改了什么见 [CHANGELOG.md](CHANGELOG.md)**（按版本记录改动和涉及的文件，实现前先看）。

> 只有设计，没有实现。本目录给出**视觉和交互的像素级规范**，覆盖 macOS 客户端、Android、iOS、服务端 Web 管理页。
> 产品行为（有哪些页面、按钮做什么）以 `docs/DESIGN.md` 为准；视觉、像素、文案 key、交互细则以本目录为准（DESIGN.md 第 2 章已指向这里）。

## 怎么看

```bash
open design/prototype/index.html      # 总览：色板、字体、间距、组件
open design/prototype/mac.html        # macOS：58 个屏幕，顶部可切换
open design/prototype/mobile.html     # Android 与 iOS 并排，42 组屏幕（?only=group,groupinfo 只看指定屏幕）
open design/prototype/web.html        # /admin 管理页
open design/prototype/blocks.html     # 消息块目录（桌面 / 移动并排）
```

顶部浮动条可以切换 **浅色 / 深色** × **不透明 / 半透明**，也可以用 URL 参数：`mac.html?theme=dark&material=translucent#group`。

## 文件

| 文件 | 内容 |
|------|------|
| `CHANGELOG.md` | 版本更新记录：每一版改了什么、改在哪些文件、实现要注意什么 |
| `tokens.json` | **唯一的 token 来源**：颜色（浅 / 深 / 半透明覆盖）、字体、字号、间距、圆角、阴影、动效、布局尺寸 |
| `tools/build_tokens.py` | 从 tokens.json 生成 `prototype/tokens.css`。改 token 只改 json，然后运行 `python3 design/tools/build_tokens.py` |
| `01-foundations.md` | 设计语言：参考来源、原则、颜色、材质、字体、间距、圆角、阴影、动效、图标、头像 |
| `02-components.md` | 组件规范：每个组件的尺寸、内边距、状态、颜色 token |
| `03-macos.md` | macOS 客户端：窗口、三栏、每个屏幕的布局和尺寸、交互、快捷键 |
| `04-mobile.md` | Android 与 iOS：共用部分和平台差异、每个屏幕、手势、通知 |
| `05-web-admin.md` | 服务端 `/admin` 页面 |
| `06-message-blocks.md` | **消息块目录**：所有消息类型（群聊跳转、选项选择、@、审批、验收、产物、任务……）及其状态，三端通用 |
| `07-copy.md` | **文案与 i18n key 对照表**（三端共用） |
| `08-interaction.md` | 焦点顺序、快捷键、无障碍、头像 / 截断 / 时间格式等细节规则 |
| `sounds/` | 提示音。**三端只用 `04-bubble-pop`（啵啵 · 气泡）**，其余是候选，规则见 01 §11 |
| `prototype/` | 可交互的 HTML 原型（`base.css` 里每一个数值都是规范的一部分） |

## 交付形式

设计稿只交付 **CSS 和 HTML 原型**。macOS 客户端是 Tauri + React，**直接复用** `tokens.css` 和 `base.css`，消息块按 `blocks.js` 的结构写成组件；Android 按同名 token 实现。`tokens.json` → `prototype/tokens.css`（生成），`prototype/base.css`（组件），`prototype/blocks.js`（消息块结构）。不提供 Rust 或 Kotlin 代码；各端实现时照着 CSS 里的数值写自己的主题，token 名保持一致（`bg.sidebar` → CSS `--bg-sidebar`、Rust `bg_sidebar`、Kotlin `bgSidebar`）。

## 参考来源与取舍

| 参考 | 借鉴 | 不照搬 |
|------|------|--------|
| **Grok Bot** | 三栏、豆豆头像、任务卡片、A/B/C 提问、Allow once / Always / Deny | 黑白反色气泡：对比太强，改成 Telegram 式淡色气泡 |
| **Telegram Desktop** | 聊天区独立底色、白色接收气泡、浅色发送气泡；连续消息合并、尾角收窄；时间嵌在气泡右下角；日期胶囊；悬停工具条 | 彩色用户名、强蓝绿色的发送气泡 |
| **Zeron**（GPUI） | 毛玻璃侧栏、按注意力排序的会话列表、工作详情放在侧栏 | — |
| **MonoCode** | composer 作为主输入、轨迹 / 终端输出的等宽深色块、标签式并行会话（Agent Computer 的标签页） | IDE 式密集布局 |
| **Apple HIG** | 系统字体、13pt 桌面正文、SF Symbols 线宽、分段控件、开关、vibrancy 材质、iOS 大标题和底部 sheet | — |
| **Material 3**（Android） | 64dp 顶栏、80dp 导航栏和 64×32 指示胶囊、56dp FAB、48dp 触控目标 | 动态取色（Material You）：品牌色固定 |
