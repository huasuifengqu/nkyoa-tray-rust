# 基线冻结（2026-11-21）

本文件是 Rust 客户端的工具链、依赖与契约事实基线。升级任何条目都要更新本文件、
重新跑契约测试与 Windows 安装验收。

## 工具链

| 项 | 值 |
| --- | --- |
| OS | Windows 11 Pro 10.0.26200 |
| Rust / Cargo | 1.100.0（`rust-toolchain.toml` 锁定） |
| 目标 | `x86_64-pc-windows-msvc` |
| 格式化 / 静态检查 | `rustfmt`、`clippy -D warnings` |

## 冻结的依赖（Cargo.lock）

| 依赖 | 版本 | 用途 |
| --- | --- | --- |
| slint / slint-build | 1.19.0 | 设置窗口 + 原生系统托盘（`system-tray`） |
| windows | 0.62.2 | Toast、凭据、快捷方式、Shell、单实例 |
| windows-registry | 0.100.0 | HKCU 自启动与暂停偏好 |
| reqwest | 0.12.28 | HTTPS（系统 TLS / Schannel，禁止重定向） |
| tokio / tokio-util | 1.54.1 / 0.7.20 | 接收任务、取消、命名管道 |
| serde / serde_json | 1.0.229 / 1.0.154 | 协议与状态序列化 |
| time | 0.3.55 | RFC3339 UTC 时间 |
| uuid | 1.29.3 | 安装标识与设备标识 |
| url | 2.5.9 | 站点规范化与 IDN |
| sha2 | 0.10.9 | profileId、契约哈希、管道名 |
| async-trait | 0.1 | 可替换的协议/存储/通知接口 |
| httpdate | 1.0.3 | `Retry-After` HTTP-date |

## 契约冻结

- OpenAPI 快照：`contracts/nkyoa-desktop-v1.openapi.json`
- SHA-256：`5c3e158b79b5dd3e043013b93b5a9f19e2df56fa8e1d22547c024d976a5461af`
- 来源清单：`contracts/source-manifest.json`（协议 v1、5 个路径、导出器 1）
- 黄金样本：`contracts/golden/*.json`（注册、设备、拉取、确认、历史过期、重同步、撤销）
- 契约测试：`crates/tray-core/tests/api_contract.rs` 校验哈希、路由、请求头/请求体与字段级拒绝面。

服务端联调仍通过隔离测试实例进行；本仓只依赖快照与黄金样本。

## 路线记录

- **Slint 1.19 覆盖 Windows 与后续 UOS 界面**。计划 §3 原定 Windows 首选
  Win32+WinRT 设置窗口；按用户 2026-11-21 决定，两平台统一使用 Slint，
  Windows 仍以 WinRT 发送 Toast、以 Shell_NotifyIcon 显示托盘（由 Slint 后端实现）。
- Slint 许可：`LicenseRef-Slint-Royalty-free-2.0`（分发时随附许可文本，见
  `packaging/windows/licenses/`）。
- UOS 预验证所需的 `winit` X11 软件渲染与 SNI 托盘后端尚未在本版启用，
  留待 R4 平台接入时验证。

## 与 .NET 版的兼容锚点

| 项 | 值 |
| --- | --- |
| 协议版本 | v1 |
| 序号 | 十进制字符串，内部非负 `i64` |
| 配置标识 | `sha256("{origin}\n{userId:N}\n{deviceId:N}")` 小写十六进制 |
| 状态文件 | `state.json` v1（`version/initial/through/ackPending/pending/resyncPending`） |
| 凭据 blob | `{"Version":1,"Origin":…,"UserId":…,"DeviceId":…,"Token":…}`（PascalCase，与 .NET 默认序列化一致） |
| 默认轮询 | 60 秒，±10% 抖动（1 秒为继续拉取，无抖动） |
| 弹窗上限 | 每页 3 个，超额合并；历史过期在续拉前过滤 |

## 资源实测（2026-11-21 本机快速抽样）

| 版本 | Private Bytes | 工作集 | 线程 | 条件 |
| --- | --- | --- | --- | --- |
| Rust Release（软件渲染，当前） | 6.3 MiB | 27.0 MiB | 12 | 启动 6 秒、设置窗口打开；关闭窗口后读数不变 |
| Rust Release（OpenGL/femtovg，早期） | 189 MiB | 86.8 MiB | 30 | 同上；GPU 驱动栈使私有字节虚高，未采用 |
| .NET 10 + Avalonia（`nkyoa-tray/artifacts/win-x64`） | 105.1 MiB | 141.4 MiB | 58 | 启动 14 秒、设置窗口打开 |

- 方法：`Get-Process` 读取 `PrivateMemorySize64` / `WorkingSet64`；同机、同一电源计划。
- 渲染选择：Slint `winit` + `renderer-software`，编译期关闭 femtovg；tokio 工作线程固定 2。
- 结论：当前版本私有字节约为 .NET 的 6%、工作集约 19%，低于计划 §7 的 60%/70% 门槛。
- 本表是快速抽样；最终预算仍按计划 §7 预热 2 分钟、采样 10 分钟、3 轮取中位数/P95。
- 分发体积：EXE 12.50 MiB；ZIP 6.56 MiB（SHA-256
  `205dcff1e00484dcdf66d0dd331c1f2a3d737f1730d82723b8522ed255a07ba2`）；
  .NET 同口径 ZIP 50.70 MiB，当前约为其 12.9%。
