# GitHub 调研：macOS 菜单栏托盘 / 桌面悬浮窗 实时仪表盘参考

调研方式：websearch 技能 + IAB（Browser Use），SERP → 打开结果页 → 阅读渲染内容，全程单 tab。
目标场景：Electron 应用，Tray 显示账户余额/用量文字摘要 + 可选悬浮窗卡片 + 字段勾选/排序/阈值变色设置。

## 1. exelban/stats（最主流的菜单栏仪表盘）

- 链接：https://github.com/exelban/stats ｜ star：42.2k ｜ 技术栈：Swift / AppKit（原生，非 Electron）
- 定位：macOS 菜单栏系统监控，每类指标（CPU/GPU/内存/磁盘/网络/电池/传感器…）是一个独立"模块"，直接把数值文字渲染进菜单栏。
- Tray 文字摘要：菜单栏里显示各模块当前读数（如 CPU 5%、网络 ↑↓），点击展开详细菜单/图表。
- 设置自定义：统一设置窗口里逐模块启用/禁用、配显示项与刷新参数；README 明确建议"关掉高开销模块来降低能耗"（即模块级开关是核心设置项）；另有桌面 Widget 需在设置中显式打开。
- 可借鉴：
  1. "模块 = 勾选项"的设置组织：一个设置页列出所有可显示字段，勾选即决定菜单栏出现哪些文字，天然实现"字段勾选"。
  2. 菜单栏文字随数据实时刷新、每字段独立开关的模式，可直接映射到我们要的"余额/用量摘要按字段勾选显示"。

## 2. zhongyang219/TrafficMonitor（悬浮窗 + 字段勾选 + 逐项配色）

- 链接：https://github.com/zhongyang219/TrafficMonitor ｜ star：46.4k ｜ 技术栈：C++ / Win32（Windows）
- 定位：桌面悬浮窗显示网速/CPU/内存，可嵌入任务栏；支持皮肤、插件、历史流量统计。
- 悬浮窗做法：启动即显示悬浮窗；悬浮窗右键 = 主菜单；"通知区图标右键菜单 → 显示任务栏窗口"在两种形态间切换；悬浮窗可拖动。
- 设置自定义：
  - 右键菜单 → "显示设置"对话框：**勾选需要显示的项目**（默认只显示网速，勾 CPU/内存等）。
  - "选项"对话框：分别设主窗口/任务栏窗口的文本颜色、字体、背景；1.72 起支持"指定每个项目的颜色"——**逐字段单独配色**。
  - 皮肤用 skin.ini/skin.xml 配色值与各项目位置、字体。
- 可借鉴：
  1. "右键菜单 → 显示设置（勾选字段）"的入口路径，正好覆盖我们"字段勾选"的交互；设置对话框和悬浮窗是两个入口但共享同一份配置。
  2. "逐字段独立文本颜色"是阈值变色的近似做法：先把字段级颜色开放出来，再叠加阈值规则变色。

## 3. getagentseal/codeburn（菜单栏显示"用量/花费"摘要，最贴近数据语义）

- 链接：https://github.com/getagentseal/codeburn ｜ star：11.3k ｜ 技术栈：Node.js / TypeScript（npx codeburn，桌面应用 + macOS 菜单栏 + Windows 托盘；README 未标注 Electron）
- 定位：本地统计 AI 编程 token 用量与花费，按项目/模型/任务拆分。
- Tray 文字摘要："今日花费"直接显示在 macOS 菜单栏时钟旁；Windows 侧是托盘显示今日成本，行为一致。
- 悬浮/详情：点击菜单栏项打开 popover，内容是主页面的"短版"——今日数值 + 周期切换（今天/本月/全部）+ 趋势 + 按模型拆分；桌面端另有边缘"Capacity Dock"环形指示与 hover 出现的 glance 卡片。
- 设置自定义：可配置订阅计划（`codeburn plan set claude-max`），菜单栏/面板会从"花了多少"升级为"额度用了多少、本月是否会超"——即**额度阈值语义**；每个数字都可点击下钻。
- 可借鉴：
  1. 菜单栏 = 一行短摘要（今日余额/用量），点开 popover 放完整仪表——正好是"Tray 摘要 + 悬浮卡片"的分层。
  2. "计划额度 → 用量进度/超支预警"直接对应我们的阈值变色：接近额度变橙、超出变红，比裸数值更有信息量。

## 4. gitify-app/gitify（同为 Electron 的菜单栏应用结构范本）

- 链接：https://github.com/gitify-app/gitify ｜ star：5.4k ｜ 技术栈：Electron + React + TypeScript + Tailwind（electron-builder 打包，topic 含 `electron-menubar`，用自家 fork https://github.com/gitify-app/electron-menubar）
- 定位：菜单栏统一显示各 Git 平台未读通知数（数字型摘要），macOS/Windows/Linux 三端。
- Tray 做法：托盘图标旁显示未读计数；macOS 以 agent 方式启动、不占 Dock；支持多账户（多 forge adapter 模式）。
- 设置自定义：设置页含过滤器（哪些仓库/类型计入计数）、主题、菜单栏显示开关；通知计数 → 点击打开列表窗口。
- 可借鉴：
  1. 同栈可直接抄结构：electron-menubar 模式 = Tray + 点击弹出 BrowserWindow（popover），窗口定位跟随托盘图标（positioner）。
  2. "过滤器决定托盘计数"与我们"字段勾选/排序决定托盘摘要内容"同构：设置里维护一份字段列表，主进程按其渲染 Tray 文案。

## 5. trcnvrl/electron（同栈悬浮卡片 + 展开仪表盘，star 很低仅作结构参考）

- 链接：https://github.com/trcnvrl/electron ｜ star：0（新项目，注意成熟度风险）｜ 技术栈：Electron + React + TypeScript + Tailwind + Framer Motion，指标用 systeminformation
- 定位：常驻托盘的悬浮 widget：紧凑模式只显示一个核心指标，点击展开毛玻璃完整仪表盘（CPU/RAM/磁盘/网络/GPU/温度）。
- 悬浮窗做法：无边框、置顶（高 Z-order，可盖过任务栏）；拖动后记住位置；点 widget 展开/收起 dashboard。
- 设置自定义：settings 界面里 toggle 各指标 + 系统选项；2 秒轮询节流；开机自启。
- 可借鉴：
  1. "紧凑卡片 ↔ 展开仪表盘"的两级视图，可复用为悬浮窗默认显示余额摘要、点击展开字段卡片。
  2. 拖动 + 位置持久化 + always-on-top + 点击收起，是 Electron 悬浮窗的最小完整交互集。

## 不适用我们的部分

- **数据源不同**：stats/TrafficMonitor/Chapi 类读本地系统指标（CPU/网速/温度），codeburn 读本地 session 文件；我们是账户 API 拉取余额/用量，轮询与缓存策略不能照搬（API 有配额，需节流）。
- **平台/技术栈不同**：stats 是 Swift/NSStatusItem、TrafficMonitor 是 Win32 任务栏嵌入与 bitmap 皮肤（skin.ini），这两者在 Electron macOS Tray（`Tray.setTitle` 文字 + `setImage` 图标）里没有对应物，只能借交互不能借实现；TrafficMonitor 的"嵌入任务栏"在 macOS 上不存在。
- **悬浮窗实现细节**：TrafficMonitor 的皮肤体系（自定义 bmp/png 皮肤 + 逐项目坐标）对 Web 渲染的 Electron 悬浮窗过重；我们用 HTML/CSS 即可，不必复刻。
- **Gitify 无悬浮窗/无数值仪表**，只借它的 Tray+popover 与设置结构；**trcnvrl/electron star 为 0**、面向 Windows，成熟度不足，只借交互不作架构依据。
- **Chapi-creator/system-monitor**（Rust+Tauri+Win32，0★）也查过：其"阈值 + 反刷屏冷却 + settings.json 持久化"思路可参考，但栈不同（Tauri），故未列入主参考。

---
调研日期：2026-09-30；star 数为当日页面读数。
