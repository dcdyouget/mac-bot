# 记忆原型独立验收

日期：2026-10-13

## 结果

- `design/tools/build_tokens.py` 运行成功；生成后 `design/prototype/tokens.css` 无 Git diff。
- `git diff --check -- design/prototype/mac.html design/prototype/mobile.html` 通过。
- 桌面 `#set_memory` 和 `#set_memory_onboarded`：已填字段只显示值，没有叠加占位文案。条目来源和日期时间完整；手动来源显示 `你手动添加 · 10/06 09:15`，Bot 来源显示「产品 记下 · 10/05 18:33」。
- 桌面配额满态：添加按钮带 `disabled`，进度条为 danger 色，错误提示可见。
- 移动端 `?only=memory,memoryonboarded`：档案已填态的称呼、语言、身份都正常；curator、用户手动和旧 bot 来源均显示时间。空档案身份占位文案最初在固定 34dp 字段中换行溢出；主 agent 调整为单行省略后复验通过，右侧显示省略号，不再挤压时区行。
- 当前记忆页截图的卡片圆角、柔和描边、石板蓝强调色符合 v1.2 的卡片和颜色 token 观感；未发现本次记忆改动破坏这些视觉效果。

## 截图

- [mac-memory-top.png](mac-memory-top.png)：桌面未完成态，顶部说明与档案卡完整可见。
- [mac-memory-onboarded.png](mac-memory-onboarded.png)：桌面已填态。
- [mac-memory-quota-full.png](mac-memory-quota-full.png)：桌面配额满态。
- [mobile-memory-final.png](mobile-memory-final.png)：移动端空档案和已填档案，包含省略修复后的验收画面。
- [mobile-memory-filled.png](mobile-memory-filled.png)：移动端已填态。
- [mobile-memory.png](mobile-memory.png) 与 [mobile-memory-refreshed.png](mobile-memory-refreshed.png)：省略修复前的诊断截图；后者可看到字段溢出与截断问题。

截图通过本机 Chrome 无头模式生成。早期 `mac-memory.png` 截图使用原型路由的默认滚动位置；最终顶部核验以 `mac-memory-top.png` 为准。

## 范围

本轮只验证原型和 token 生成，不改产品代码。视觉核验覆盖记忆页，不是全量逐屏 v1.2 回归。
