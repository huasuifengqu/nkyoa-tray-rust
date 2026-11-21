# 构建与验收

工作目录：本仓根。工具链由 `rust-toolchain.toml` 固定（Rust 1.100.0）。
打包与安装脚本请使用 PowerShell 7（`pwsh`）；Windows PowerShell 5.1 无法正确读取
这些无 BOM 的 UTF-8 中文脚本。

## 常规

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## 发布与打包

```powershell
./packaging/windows/build.ps1
```

脚本执行格式化检查、Clippy、全部测试和锁定依赖的 x64 Release 构建。
发布构建静态链接 MSVC C 运行库，生成 `artifacts/win-x64` 与
`artifacts/nkyoa-tray-rust-win-x64.zip`，并输出 ZIP 大小和 SHA-256。

### 生成中文 EXE 安装包

在构建机安装 [Inno Setup 7.1 或更新版本](https://jrsoftware.org/isdl.php) 后执行：

```powershell
./packaging/windows/build.ps1 -Installer
./packaging/windows/build.ps1 -Installer -IsccPath 'C:\Tools\Inno Setup 7\ISCC.exe'
```

生成物为 `artifacts/nkyoa-tray-rust-win-x64-setup.exe`。目标是 Windows 11 x64，
向导按当前用户安装并注册开始菜单、Toast 和协议激活。终端用户可直接运行此文件。
安装包默认未签名，正式分发时应使用组织的代码签名流程。
商业构建请核对 [Inno Setup 的商业许可说明](https://jrsoftware.org/inno-setup/purchase)。

### 安装、升级与卸载

双击安装包并按向导完成。升级或重新安装时，从托盘退出客户端，再运行对应版本的安装包。
从 Windows「设置 → 应用 → 已安装的应用」卸载，或运行安装目录中的卸载程序。
卸载会移除程序、自启动项和桌面集成，保留本机绑定、接收进度和暂停偏好。
需要注销 OA 设备时，请在卸载前先从客户端解除绑定。

### 使用 PowerShell 脚本安装

脚本适用于直接使用 ZIP 或发布目录进行维护的场景。通过 EXE 安装的版本使用 Windows 应用列表卸载。

```powershell
./packaging/windows/Install-NkyoaTray.ps1 -PackageDirectory artifacts/win-x64
```

安装到 `%LOCALAPPDATA%\Programs\Nkyoa.Tray.Rust`，创建开始菜单快捷方式，
并注册 AUMID `NKYOA.Tray.Rust.DesktopNotifications` 与协议
`nkyoa-notify-rust`。安装时启用自启动：追加 `-EnableAutostart`。

升级前先从托盘退出，再执行同一条安装命令；绑定、接收进度与暂停设置保留。

卸载前从托盘退出：

```powershell
./packaging/windows/Install-NkyoaTray.ps1 -Uninstall
```

默认保留 `%LOCALAPPDATA%\Nkyoa\Tray.Rust` 中的绑定与进度；追加
`-ClearBinding` 会先尝试撤销服务器设备，再清理数据目录。服务器不可达时
退出码为 5，并提示到 OA 安全中心核对设备。

## 手工验收清单（Windows 11 x64）

安装后逐项核对：

1. 开始菜单启动进入设置窗口；中文显示、DPI 缩放、输入法正常。
2. 托盘图标出现；左键打开设置；菜单包含状态、打开 OA、设置、暂停/恢复、
   本机测试通知、退出。
3. “发送系统测试通知”在通知中心出现；点击后打开已绑定的 HTTPS OA 站点。
   命令行等价检查：`nkyoa-tray.exe --selftest-notification` 应退出 0。
4. 通知中心点击在**进程运行**与**进程已退出**两种状态都只携带配置标识；
   绑定匹配才打开 OA。
5. 关闭设置窗口后进程继续接收；再次启动不产生第二实例（转发激活）。
6. 自启动开关写入 HKCU Run，重启登录后以 `--background` 启动。
7. 暂停弹窗后仍同步与确认；恢复后不补弹历史。
8. 解除绑定后凭据与待展示页清理；服务器设备撤销；失败时给出状态提示。
9. 资源管理器重启后托盘图标自动恢复（Slint 后端处理 TaskbarCreated）。
10. 睡眠唤醒后继续轮询；退出后无进程残留。
11. 从 Windows 应用列表卸载后检查程序和桌面集成已移除、状态和绑定仍保留。脚本安装另验证 `-ClearBinding`。
12. `--check-instance`：无实例返回 0，有实例返回 3。

Windows 10 22H2 未在本机验证；纳入支持范围前须在同版镜像完成第 1–12 项。
UOS 真机验收按计划 §9 在 R4 阶段进行。
