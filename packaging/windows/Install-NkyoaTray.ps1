[CmdletBinding(SupportsShouldProcess)]
param(
    [string] $PackageDirectory,
    [switch] $EnableAutostart,
    [switch] $Uninstall,
    [switch] $ClearBinding
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows -and [Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
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

function Invoke-Installed([string] $Executable, [string[]] $Flags) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Flags -WindowStyle Hidden -PassThru -Wait
    $process.ExitCode
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
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw '未找到当前用户安装的客户端。'
    }
    if (-not $PSCmdlet.ShouldProcess($install, '卸载当前用户客户端')) { return }
    if ($ClearBinding) { Assert-OwnedDirectory $data $dataParent }
    $flags = @('--remove-integration')
    if ($ClearBinding) { $flags += '--clear-binding' }
    $result = Invoke-Installed $exe $flags
    if ($result -eq 3) { throw '请先从托盘退出客户端，再重试卸载。' }
    if ($result -notin @(0, 5)) { throw "清理 Windows 集成失败，退出码 $result。" }
    Remove-OwnedDirectory $install $programs
    if ($ClearBinding) { Remove-OwnedDirectory $data $dataParent }
    if ($result -eq 5) { Write-Warning '本机绑定已移除。请到 OA 安全中心核对服务器设备记录。' }
    return
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
