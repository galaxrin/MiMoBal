# MiMoBal（mimo-dash）

MiMo 余额 / Token Plan 桌面仪表盘：Electron 托盘常驻应用，提供**悬浮窗卡片**与**托盘摘要**两种"扫一眼"入口，支持字段勾选/排序、阈值变色与系统通知。

数据来自 MiMo 平台接口（`platform.xiaomimimo.com/api/v1`）与桌面端订阅接口（`mimo-server`，sid=mimopc），**复用 MiMo Desktop 的小米账号登录态**，无需重复输入密码。

## 功能

- **悬浮窗**：核心大数字（可自定义两个核心位）+ 7 日余额走势 + 可展开字段列表；拖动记忆位置、尺寸随内容自适应、点击数值复制、mini 单行模式
- **托盘**：macOS 在菜单栏显示文字摘要；Windows 摘要显示在悬停 tooltip 与右键菜单中，左键点击图标切换悬浮窗
- **字段系统**：13 个字段（余额 / Token Plan / 桌面端订阅），悬浮窗与托盘各持一份勾选与排序
- **阈值告警**：字段级"低于 / 高于"阈值，触发变红并发送系统通知（30 分钟限频）
- **刷新**：默认 60 秒轮询（10 秒 ~ 5 分钟可选），失败自动重试，错误态提供手动"重试"

## 运行

```bash
npm install
npm start
```

需要较新的 Node.js（`data.js` 使用 `node:sqlite`，Node ≥ 22）。前置条件：本机已安装并登录 [MiMo Desktop](https://mimo.xiaomi.com)。

## 打包（Windows）

```bash
npm run dist        # 产出 dist/MiMoDash-Setup-<version>.exe 与 dist/win-unpacked/
```

若在 **MiMo Desktop 内置运行时**下执行（其 `node.exe` 是转发壳，无法运行 electron-builder），改用系统 Node，或临时下载便携版 Node 并前置到 PATH：

```powershell
# 例：便携 Node 前置后直接调用 cli
$env:PATH = "C:\path\to\node-v22-win-x64;$env:PATH"
node node_modules/electron-builder/cli.js --win
```

图标 `icon.ico` 为 256×256 单条目（electron-builder 要求主图 ≥ 256px）。

## Windows 凭证读取原理（重点）

应用本身没有账号密码，需要从 MiMo Desktop 的存储里取 `passToken` 换平台会话。两个来源，按顺序回退（`data.js`）：

1. **Cookie 数据库**：`%APPDATA%\Xiaomi MiMo\Partitions\xiaomi-account\Network\Cookies`（旧布局在分区根）。macOS 可直接读取；**Windows 上该文件通常被 MiMo Desktop 的 Chromium Network Service 持有**，读取会失败。
2. **HTTP 缓存回退**：分区缓存 `Cache\Cache_Data\*` 中缓存的 serviceLogin 响应 JSON 回显了当前 `passToken` / `userId`。逐文件跳过被占用的块 + 异步整轮重试；解析做了大括号配对（跳过字符串内的花括号）。

登录时若服务端拒绝某枚凭证，会自动拉黑并逐枚尝试下一候选（最多 3 枚）；全部被拒时提示"在 MiMo Desktop 重新登录"。**重启或重新登录 MiMo Desktop 会刷新缓存中的凭证**。

其他途径实测不可用：API Key（sk-/tp-）访问 `platform.xiaomimimo.com/api/v1/*` 返回 401；MiMo Desktop 的本地 desktop-api（`desktop-api.json`）只有会话/消息接口，无账户数据。

## 配置

配置文件：`%APPDATA%\mimo-dash\config.json`（macOS：`~/Library/Application Support/mimo-dash/config.json`），设置页所有改动防抖自动落盘，无保存按钮。

| 键 | 说明 |
|---|---|
| `refreshSec` | 刷新间隔（≥ 10 秒） |
| `float.*` | 悬浮窗：`show` / `opacity` / `density` / `mode` / `heroIds` / `bounds` / `fields` |
| `tray.*` | 托盘：`showTitle` / `fields` |
| `thresholds` | `{ 字段id: { min?, max? } }` |

余额历史（`history.json`）本地保留 30 天，用于 sparkline。

## 目录结构

```
main.js          主进程：托盘 / 悬浮窗 / 设置窗 / IPC / 刷新与告警
fields.js        字段注册表与纯计算（computeValues / isRed）
data.js          凭证读取（Cookie → 缓存回退）与平台/订阅会话
preload.js       contextBridge 暴露 api
float.html       悬浮窗渲染
settings.html    设置页渲染
icon-win.png     Windows 托盘彩色图标（32px）
icon.ico         打包用 256px 图标
mimo_widget.py   macOS Tkinter 原型存档（已被 Electron 版取代，未适配 Windows）
github-reference.md  竞品调研笔记
```

## 故障排查

| 现象 | 处理 |
|---|---|
| 悬浮窗显示"未找到 passToken" | 确认 MiMo Desktop 已登录；Windows 上稍候会自动重试，或重启 MiMo Desktop 刷新缓存 |
| "缓存中的凭证已被服务端拒绝" | 在 MiMo Desktop 退出并重新登录 |
| 阈值通知不弹 | Windows 通知需系统通知未开启"专注助手"；未打包运行时通知归属取决于 AUMID（已设置 `com.galaxrin.mimo-dash`，安装后与快捷方式一致） |
| 托盘找不到图标 | Windows 新图标默认收进 `^` 溢出区，可拖出固定 |
| 修改了 `config.json` 不生效 | 确保是 UTF-8（无 BOM 也会被容忍）；解析失败会在控制台告警并回退默认值 |

## License

MIT
