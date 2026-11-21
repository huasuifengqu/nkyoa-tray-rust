[CmdletBinding(SupportsShouldProcess)]
param(
    [string] $PackageDirectory,
    [switch] $EnableAutostart,
    [switch] $Uninstall,
    [switch] $ClearBinding,
    [switch] $NonInteractive
)

$ErrorActionPreference = 'Stop'
# 不用 $IsWindows：它只存在于 PowerShell 7，而卸载入口由系统自带的
# Windows PowerShell 5.1 调用，5.1 下 $IsWindows 为空会误判为非 Windows。
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw '此脚本仅支持 Windows。'
}
if ($ClearBinding -and -not $Uninstall) {
    throw '-ClearBinding 只用于卸载。'
}

$local = [Environment]::GetFolderPath('LocalApplicationData')
$programs = [IO.Path]::GetFullPath((Join-Path $local 'Programs'))
$install = Join-Path $programs 'Nkyoa.Tray.Rust'
$dataParent = [IO.Path]::GetFullPath((Join-Path $local 'Nkyoa'))
$data = Join-Path $dataParent 'Tray.Rust'
$startMenu = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'Microsoft\Windows\Start Menu\Programs'
$uninstallItem = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\NKYOA.Tray.Rust'
$displayName = 'OA通知'
$uninstallScriptName = 'Install-NkyoaTray.ps1'
# 本仓早期品牌叫「nkyoa 桌面通知 (Rust)」；改名后旧版留下的快捷方式不在
# 新命名的清理范围内，安装与卸载时都要显式移除。
$legacyShortcutName = 'nkyoa 桌面通知 (Rust).lnk'

function Invoke-Installed([string] $Executable, [string[]] $Flags) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Flags -WindowStyle Hidden -PassThru -Wait
    $process.ExitCode
}

function Remove-LegacyIntegration {
    $legacy = Join-Path $startMenu $legacyShortcutName
    if (Test-Path -LiteralPath $legacy -PathType Leaf) {
        Remove-Item -LiteralPath $legacy -Force
    }
}

function Register-UninstallEntry([string] $ClientExe, [string] $ScriptPath) {
    # 只有 HKCU 的 Uninstall 项才会出现在「设置 → 应用 → 已安装的应用」。
    # Inno 安装包由 Inno 自己写同位置的 {AppId}_is1 项，两种渠道共用此机制。
    $powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $command = '"{0}" -NoProfile -ExecutionPolicy Bypass -File "{1}" -Uninstall' -f $powershell, $ScriptPath
    $version = (Get-Item -LiteralPath $ClientExe).VersionInfo.ProductVersion
    if (Test-Path -LiteralPath $uninstallItem) {
        Remove-Item -LiteralPath $uninstallItem -Recurse -Force
    }
    New-Item -Path $uninstallItem -Force | Out-Null
    $strings = [ordered]@{
        DisplayName = $displayName
        DisplayIcon = $ClientExe
        InstallLocation = $install
        UninstallString = $command
        QuietUninstallString = $command
    }
    if (-not [string]::IsNullOrWhiteSpace($version)) {
        $strings['DisplayVersion'] = $version
    }
    foreach ($name in $strings.Keys) {
        New-ItemProperty -Path $uninstallItem -Name $name -Value $strings[$name] -PropertyType String -Force | Out-Null
    }
    New-ItemProperty -Path $uninstallItem -Name 'NoModify' -Value 1 -PropertyType DWord -Force | Out-Null
    New-ItemProperty -Path $uninstallItem -Name 'NoRepair' -Value 1 -PropertyType DWord -Force | Out-Null
}

function Remove-UninstallEntry {
    if (Test-Path -LiteralPath $uninstallItem) {
        Remove-Item -LiteralPath $uninstallItem -Recurse -Force
    }
}

function Show-UninstallError([string] $Message) {
    # 从「设置」点卸载时控制台窗口一闪而过，错误必须用消息框才看得见。
    [Console]::Error.WriteLine($Message)
    if ($NonInteractive) { return }
    try {
        Add-Type -AssemblyName Microsoft.VisualBasic
        [void] [Microsoft.VisualBasic.Interaction]::MsgBox($Message, 'OKOnly,Exclamation', $displayName)
    } catch {
        # 消息框不可用时 stderr 已经给出原因。
    }
}

function Assert-OwnedDirectory([string] $Path, [string] $Parent) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetDirectoryName($absolute) -ine $Parent) {
        throw "安装操作路径超出预期目录。"
    }
    foreach ($candidate in @($Parent, $absolute)) {
        if (Test-Path -LiteralPath $candidate) {
            $item = Get-Item -LiteralPath $candidate -Force
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw "安装目录或其父目录是重解析点。"
            }
        }
    }
}

function Remove-OwnedDirectory([string] $Path, [string] $Parent) {
    Assert-OwnedDirectory $Path $Parent
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
}

Assert-OwnedDirectory $install $programs
$exe = Join-Path $install 'nkyoa-tray.exe'
if ($Uninstall) {
    $installed = Test-Path -LiteralPath $exe -PathType Leaf
    if (-not $installed -and -not (Test-Path -LiteralPath $install) -and
        -not (Test-Path -LiteralPath $uninstallItem)) {
        throw '未找到当前用户安装的客户端。'
    }
    try {
        if (-not $PSCmdlet.ShouldProcess($install, '卸载当前用户客户端')) { return }
        if ($ClearBinding) { Assert-OwnedDirectory $data $dataParent }
        # 从「设置」调用时当前目录可能就在安装目录里，先移开才能删除它。
        Set-Location -LiteralPath $local
        $result = 0
        if ($installed) {
            $flags = @('--remove-integration')
            if ($ClearBinding) { $flags += '--clear-binding' }
            $result = Invoke-Installed $exe $flags
            if ($result -eq 3) { throw '请先从托盘退出客户端，再重试卸载。' }
            if ($result -notin @(0, 5)) { throw "清理 Windows 集成失败，退出码 $result。" }
        }
        Remove-LegacyIntegration
        Remove-OwnedDirectory $install $programs
        if ($ClearBinding) { Remove-OwnedDirectory $data $dataParent }
        Remove-UninstallEntry
        if ($result -eq 5) { Write-Warning '本机绑定已移除。请到 OA 安全中心核对服务器设备记录。' }
        return
    }
    catch {
        Show-UninstallError $_.Exception.Message
        exit 1
    }
}

if ([string]::IsNullOrWhiteSpace($PackageDirectory)) { throw '请传入解压后的发布目录。' }
$source = (Resolve-Path -LiteralPath $PackageDirectory).ProviderPath.TrimEnd('\')
if ($source -ieq $install -or $source.StartsWith($install + '\', [StringComparison]::OrdinalIgnoreCase) -or
    $install.StartsWith($source + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw '请使用安装目录之外的发布包。'
}
foreach ($file in @('nkyoa-tray.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $file) -PathType Leaf)) {
        throw "发布目录缺少 $file。"
    }
}
if (-not $PSCmdlet.ShouldProcess($install, '安装或升级当前用户客户端')) { return }

$check = if (Test-Path -LiteralPath $exe -PathType Leaf) { $exe } else { Join-Path $source 'nkyoa-tray.exe' }
$probe = Invoke-Installed $check @('--check-instance')
if ($probe -eq 3) { throw '请先从托盘退出客户端，再重试安装。' }
if ($probe -ne 0) { throw "检查客户端进程失败，退出码 $probe。" }

[void] [IO.Directory]::CreateDirectory($programs)
$stage = Join-Path $programs ('Nkyoa.Tray.Rust.stage.' + [Guid]::NewGuid().ToString('N'))
$backup = Join-Path $programs ('Nkyoa.Tray.Rust.backup.' + [Guid]::NewGuid().ToString('N'))
Assert-OwnedDirectory $stage $programs
Assert-OwnedDirectory $backup $programs
[void] [IO.Directory]::CreateDirectory($stage)
$saved = $false
$deployed = $false
try {
    Get-ChildItem -LiteralPath $source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse -Force
    }
    # 卸载入口指向安装目录里的这份脚本，源发布目录随时可以删除。
    Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $stage $uninstallScriptName) -Force
    if (Test-Path -LiteralPath $install) {
        [IO.Directory]::Move($install, $backup)
        $saved = $true
    }
    [IO.Directory]::Move($stage, $install)
    $deployed = $true
    $flags = @('--install-integration')
    if ($EnableAutostart) { $flags += '--autostart' }
    $result = Invoke-Installed $exe $flags
    if ($result -ne 0) { throw "注册 Windows 通知集成失败，退出码 $result。" }
    Remove-LegacyIntegration
    # Inno 安装包与本脚本共用安装目录；脚本接管后移除 Inno 的卸载入口，
    # 否则「设置」里会留下一个指向已不存在 unins000.exe 的失效项。
    $innoItem = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{75C3C10F-B930-4D1E-8380-0CB7A59CC819}_is1'
    if (Test-Path -LiteralPath $innoItem) {
        Remove-Item -LiteralPath $innoItem -Recurse -Force
    }
    Register-UninstallEntry $exe (Join-Path $install $uninstallScriptName)
}
catch {
    if ($deployed -and (Test-Path -LiteralPath $exe -PathType Leaf) -and -not $saved) {
        [void] (Invoke-Installed $exe @('--remove-integration'))
    }
    if ($deployed) { Remove-OwnedDirectory $install $programs }
    if ($saved) { [IO.Directory]::Move($backup, $install) }
    throw
}
finally {
    Remove-OwnedDirectory $stage $programs
}
if ($saved) { Remove-OwnedDirectory $backup $programs }
Write-Output $exe
