# 架构与实现边界

## 分层

```text
tray-windows（进程入口）
  ├─ Slint AppWindow / AppTray（ui/app.slint）
  ├─ controller：UI ↔ 客户端粘合、脏标记刷新、诊断导出
  ├─ platform：自启动、暂停偏好、打开 OA、托盘可用性
  ├─ credential：Windows 凭据管理器（与 .NET blob 兼容）
  ├─ notifier：WinRT Toast + 受控协议激活
  ├─ shell：开始菜单快捷方式、AUMID、协议注册
  ├─ single_instance：共享模式 0 的实例锁 + 命名管道激活
  └─ log：滚动、脱敏日志
tray-core
  ├─ protocol：常量、DTO、HTTP 传输、信封/字段校验
  └─ reception：状态机、JSON 持久化、客户端生命周期
```

`tray-core` 不引用 UI 或平台 SDK；平台能力全部通过 `NotificationApi`、
`CredentialStore`、`ClientStateStore`、`SystemNotifier` 四个 trait 注入，
单元测试可完全替换。

## 并发模型

- Slint 事件循环、窗口与托盘只在主线程创建与修改。
- tokio 多线程运行时承载 HTTP、计时器、命名管道监听与转发。
- UI 订阅通过 `Arc<AtomicBool>` 脏标记 + Slint 定时器（150ms）刷新；
  跨线程只传 `slint::Weak`，Slint 保证异线程 `upgrade` 返回 `None`。
- 生命周期命令（start/bind/unbind/stop）由 `tokio::sync::Mutex` 串行化；
  绑定切换先取消并等待旧接收任务，迟到响应不会写入新状态。

## 接收状态机

与 .NET 版逐条对齐：

1. 读取状态文件（缺失→按设备初始游标；损坏→标记并走恢复）。
2. 每次失败后的重试先核验设备，捕获设备撤销、流重置与名额变化。
3. 设备初始游标前进→直接采用；后退→记录待重同步原因（历史过期/流变化）。
4. 拉取到的页先原子落盘 `ackPending + pending`，落盘成功才发送确认。
5. 展示阶段按 `processed/accepted` 做检查点；崩溃后从检查点继续。
6. 单页最多 3 个弹窗；超过时最后一条合并为“还有 N 条”。
7. 历史过期且本机状态损坏时，用探测+步长搜索找回仍保留的前缀，避免整段重同步。
8. 401 撤销时清空待展示页，不重放已确认内容。

## 持久化与兼容

- 状态目录：`%LOCALAPPDATA%\Nkyoa\Tray.Rust\state\<profileId>\state.json`。
- 安装标识：`...\state\installation-id`，并发首次创建以硬链接保证唯一赢家。
- 写入：同目录临时文件 + `sync_all` + 原子替换；损坏副本只保留固定标记。
- `resyncPending` 只允许 `null/0/1`；未来版本（`version > 1`）拒绝覆盖。
- 凭据 blob 与 .NET 版同形；`profileId` 计算与 .NET `AbsoluteUri` 输入逐字一致。

## 进程表面

| 参数 | 行为 | 退出码 |
| --- | --- | --- |
| 无参数 | 显示设置窗口 | 事件循环结束 |
| `--background` | 托盘启动 | 0 / 已有实例 0 |
| `--install-integration [--autostart]` | 注册快捷方式/AUMID/协议，可选自启动 | 0 / 4 |
| `--remove-integration [--clear-binding]` | 清理集成；可选撤销并清凭据 | 0 / 3 运行中 / 4 / 5 服务器不可达 |
| `--check-instance` | 探测单实例 | 0 空闲 / 3 运行中 |
| `--selftest-notification` | 本机发送一条测试 Toast（不建实例） | 0 系统接受 / 1 未接受 |
| `nkyoa-notify-rust://open/<id>` | 转发激活 | 0 / 2 |

## 安全要点

- 错误对象只含问题分类，不含服务器 `detail`；`Debug` 对令牌与配对码脱敏。
- Toast XML 只写固定 `duration="long"`、AUMID、受控协议 URI 与转义后的标题/正文。
- 协议 URI 只接受 64 位小写十六进制；管道帧最多 69 字节、2 秒超时。
- 凭据读写通过系统 API，内存副本用 `zeroize` 清零；`DeviceToken`/`PairingCode`
  在 Drop 时清零，凭据 blob 与配对码的临时 String 也在使用后清零。
- 服务器地址仅 HTTPS 根路径；HTTP 客户端禁用重定向与 Cookie。
- 传输错误分类：连接失败默认按 `Offline` 可重试；只有 source 链出现
  `native_tls::Error` 或证书文本才归 `Certificate`；响应体读取中断归 `Offline`，
  与 .NET 的 `IOException` 口径一致，弱网抖动不会退出接收循环。
- 已知残留：命名管道按用户配置目录派生，使用创建者的默认 DACL
  （本用户/System/Administrators）。多用户同机场景下管道名被抢占只能造成
  激活失败或 DoS，不泄露凭据；后续可改为显式 DACL 的 `CreateNamedPipeW`。

## 后续（UOS）

`tray-core` 无需改动即可复用。Linux 入口需实现：
`org.freedesktop.secrets` 凭据、`org.freedesktop.Notifications` 通知、
XDG 自启动、`.desktop` 与 deb 打包；Slint 的托盘后端在 Linux 走
StatusNotifierItem，计划 §9 的 R0 门槛仍需在目标 ARM64 镜像验证。
