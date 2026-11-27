# MiMoBal

MiMo 余额 / Token Plan 桌面小工具。**macOS / Windows 双端**，托盘常驻，悬浮窗让你一眼看到余额和用量，不用打开网页。

技术栈：**Rust + Tauri 2**（悬浮窗 / 设置界面复用 `frontend/` 下的 HTML，业务逻辑全在 `src-tauri/src/`）。

## 功能

- **悬浮窗**：核心大数字 + 字段列表，拖动记忆位置，点击数值可复制，支持迷你单行模式
- **托盘**：悬停查看摘要，右键菜单；左键打开设置；菜单里可切换悬浮窗显隐
- **字段自选**：余额、套餐、用量、订阅等字段自由勾选和排序
- **阈值提醒**：字段低于/高于设定值时变红，并发送系统通知
- **自动刷新**：默认 60 秒，可在设置中调整（10 秒 ~ 5 分钟）；连续失败自动退避，网络恢复即刻重试
- **图标微调**：菜单栏图标位置可在设置里垂直微调（±3pt）
- **单实例**：重复启动只会聚焦已有实例的设置窗

## 平台差异

| 能力 | macOS | Windows |
|---|---|---|
| 入口 | 菜单栏图标（无 Dock） | 系统托盘图标，左键=设置 |
| 文字摘要 | 菜单栏标题（过长自动截断） | 悬停 tooltip + 右键菜单条目 |
| 悬浮窗透明度 | 真透明（透出桌面） | 不透明底 + 底色深浅滑条 |
| 安装包 | `MiMoBal_*.dmg`（x64 / aarch64） | `MiMoBal_*-setup.exe` / `MiMoBal_*.msi` |
| 凭证来源 | MiMo Desktop Cookie / 缓存 | 同左（Cookie 被占用时走缓存回退） |

## 安装使用

### Windows

1. 确保已安装并登录 [MiMo Desktop](https://mimo.xiaomi.com)
2. 到 [Releases](https://github.com/galaxrin/MiMoBal/releases) 下载 `MiMoBal_*-setup.exe`，双击安装（可选桌面快捷方式 / 开机自启）
3. 启动后在系统托盘找到 MiMo 图标

### macOS

1. 确保已安装并登录 MiMo Desktop
2. 打开 `MiMoBal_*.dmg`（Intel 选 x64、Apple Silicon 选 aarch64），把应用拖到「应用程序」，首次打开若被 Gatekeeper 拦截：右键 → 打开（未签名构建）
3. 启动后在**菜单栏**找到 MiMo 图标

**常用操作**

| 想做什么 | 怎么做 |
|---|---|
| 看余额 / 用量 | 看悬浮窗；或悬停托盘/菜单栏图标 |
| 改显示哪些字段 | 托盘左键（Win）/ 右键菜单 → 设置 → 悬浮窗 / 托盘栏 |
| 显示 / 隐藏悬浮窗 | 设置开关；或托盘/菜单栏右键菜单 |
| 数据不对 | 悬浮窗或设置里点「重试」 |
| 提示登录失效 | 在 MiMo Desktop 退出并重新登录，再点「重试」 |

## 从源码运行

需要 [Rust 工具链](https://rustup.rs)（`rustup`）与 Node.js 22+（仅用于 Tauri CLI）。

```bash
npm install
npm run dev      # 当前平台开发运行（tauri dev）
npm run build    # 打当前平台安装包（dmg / exe）
```

```bash
cd src-tauri
cargo test       # 字段计算 / 配置合并 / 凭证解析单测
cargo check
```

## 代码结构

```
frontend/          # 悬浮窗 / 设置界面（HTML，经 bridge.js 对接 Rust IPC）
src-tauri/src/
  main.rs          # 托盘、悬浮窗、刷新循环、IPC、阈值通知
  data.rs          # 凭证读取（Cookie 库 → HTTP 缓存回退）+ 小米 SSO + 平台 API
  fields.rs        # 字段注册表与纯计算（阈值判红等）
  config.rs        # 配置加载 / 合并 / 旧版迁移
```

macOS 打包需本机 `iconutil`（系统自带）；图标用 `npx tauri icon <png>` 重新生成到 `src-tauri/icons/`。

## License

MIT
