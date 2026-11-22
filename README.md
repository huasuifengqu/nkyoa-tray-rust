# nkyoa-tray-rust：nkyoa 桌面通知与托盘客户端（Windows / Slint）

按 `Rust版本可行性与实施计划.md` 打造的独立 Rust 客户端。首版面向 Windows 11 x64，
UI 路线按用户决定采用 **Slint 1.19**（计划原文 Windows 首选 Win32；Slint 同时覆盖
后续 UOS 的 X11/SNI 路线）。协议、接收状态机、持久化与设置行为与 `nkyoa-tray`
（.NET 10 + Avalonia）对齐，并保留独立回归测试。

## 当前状态

| 能力 | 状态 |
| --- | --- |
| 协议 v1 DTO、HTTPS、信封/序号/时间校验 | 已实现 |
| 绑定、设备核验、拉取、确认、重同步、自撤销 | 已实现 |
| 接收状态机（页内游标、损坏恢复、历史过期、流变化） | 已实现 |
| 原子持久化、安装标识并发创建、损坏标记 | 已实现 |
| 每页最多 3 个弹窗、超额合并、过期过滤、暂停继续同步 | 已实现 |
| 流程标题/流转意见预览（设备开关；仅内存与 Toast，落盘通用文案） | 已实现 |
| Slint 设置窗口（中文、DPI、密码框、解除确认） | 已实现 |
| Slint 原生系统托盘（Shell_NotifyIcon，含任务栏重建） | 已实现 |
| WinRT Toast + AUMID + 受控协议点击 | 已实现 |
| 当前用户自启动、暂停偏好、单实例、命名管道激活 | 已实现 |
| 当前用户安装/升级/卸载 EXE 与脚本 | 已实现 |
| UOS ARM64 平台接入 | 待后续（Slint 托盘后端已具备 SNI 支持） |

## 布局

```text
contracts/                 OpenAPI 快照、来源清单、黄金样本、状态 schema
crates/tray-core/          协议、HTTPS、接收状态机、持久化（无 UI/平台依赖）
crates/tray-windows/       Slint 界面、托盘、Toast、凭据、单实例、进程入口
packaging/windows/         构建、安装/卸载与许可输入
docs/                      基线、构建、架构与契约来源
```

## Windows 11 x64 安装

双击 `nkyoa-tray-rust-win-x64-setup.exe`，按中文向导安装。安装无需管理员权限，
完成后可从开始菜单打开 `OA通知`。需要随登录启动时，在客户端设置中开启。

升级前从托盘退出，再运行新版安装包。卸载入口在 Windows「设置 → 应用 → 已安装的应用」，
卸载保留已有绑定和接收进度。如需撤销设备，请先在客户端解除绑定。

## 开发与打包

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --workspace --locked
pwsh -File packaging/windows/build.ps1
pwsh -File packaging/windows/build.ps1 -Installer
```

生成 EXE 安装包需要 Inno Setup 7.1 或更新版本。[构建与脚本安装说明](docs/BUILD.md)

## 试点隔离

按计划 §6，本版与 .NET 版并行使用独立的名称与路径，避免互相覆盖：

| 项 | Rust 试点值 |
| --- | --- |
| 安装目录 | `%LOCALAPPDATA%\Programs\Nkyoa.Tray.Rust` |
| 数据目录 | `%LOCALAPPDATA%\Nkyoa\Tray.Rust` |
| AUMID | `NKYOA.Tray.Rust.DesktopNotifications` |
| 激活协议 | `nkyoa-notify-rust://open/<profileId>` |
| 自启动值 | `NKYOA.Tray.Rust` |
| 凭据目标 | `NKYOA.Tray.Rust/DesktopNotifications.v1` |
| 开始菜单快捷方式 | `OA通知.lnk` |

正式切换时需同步更新品牌常量与安装器中的 Windows 集成配置，并完成计划 §6 的双向兼容核验。

## 资源

Release 版单文件约 13 MiB。本机快速抽样（设置窗口打开）：
Private Bytes 6.3 MiB、工作集 27.0 MiB、12 线程；同机 .NET 10 + Avalonia 基线为
105.1 MiB / 141.4 MiB / 58 线程。详见 [基线文档](docs/BASELINE.md)。

## 安全边界

- 设备令牌只存 Windows 凭据管理器，日志/诊断/错误不包含令牌、校验码或通知正文。
- Toast 点击只携带 64 位小写十六进制配置标识，不接受任意 URL、命令或查询参数。
- 单实例锁与 .NET 语义一致（共享模式 0），命名管道按用户派生且限制帧长。
- 服务器地址只接受 HTTPS 根路径；请求禁止自动重定向。

## 文档

- [基线冻结](docs/BASELINE.md)
- [构建与验收](docs/BUILD.md)
- [架构与实现边界](docs/ARCHITECTURE.md)
- [协议快照来源](docs/CONTRACT-SOURCE.md)
