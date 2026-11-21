[CmdletBinding()]
param(
    [string] $OutputDirectory = 'artifacts/win-x64',
    [switch] $Installer,
    [string] $IsccPath
)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$artifacts = Join-Path $root 'artifacts'
$output = [IO.Path]::GetFullPath($OutputDirectory, $root)
$outputLicenses = Join-Path $output 'licenses'
if ([IO.Path]::GetDirectoryName($output) -ine $artifacts) {
    throw '输出目录必须是 artifacts 的直接子目录。'
}
foreach ($directory in @($artifacts, $output, $outputLicenses)) {
    $item = Get-Item -LiteralPath $directory -Force -ErrorAction SilentlyContinue
    if ($null -ne $item -and
        (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint))) {
        throw "无法使用输出目录：$directory"
    }
}

function Get-CargoMetadata {
    # cargo 输出 UTF-8；中文 Windows 的控制台代码页是 936，直接捕获会把
    # Cargo.toml 里的中文 description 解成乱码并破坏 JSON，故以 UTF-8 直读管道。
    $startInfo = [Diagnostics.ProcessStartInfo]::new('cargo', 'metadata --no-deps --format-version 1 --locked')
    $startInfo.WorkingDirectory = $root
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $startInfo.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $process = [Diagnostics.Process]::Start($startInfo)
    # 先异步收 stderr，避免管道写满后与 stdout 互锁。
    $errorTask = $process.StandardError.ReadToEndAsync()
    $json = $process.StandardOutput.ReadToEnd()
    $errorText = $errorTask.GetAwaiter().GetResult()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        throw "cargo metadata 失败：$errorText"
    }
    return $json | ConvertFrom-Json
}

if ($Installer) {
    if ([string]::IsNullOrWhiteSpace($IsccPath)) {
        $IsccPath = @(
            (Get-Command ISCC.exe -CommandType Application -ErrorAction SilentlyContinue).Source
            (Join-Path $env:ProgramFiles 'Inno Setup 7/ISCC.exe')
            (Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 7/ISCC.exe')
        ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Select-Object -First 1
    }
    if ([string]::IsNullOrWhiteSpace($IsccPath) -or -not (Test-Path -LiteralPath $IsccPath -PathType Leaf)) {
        throw '请安装 Inno Setup 7.1 或使用 -IsccPath 指定 ISCC.exe。'
    }
    $IsccPath = (Resolve-Path -LiteralPath $IsccPath).ProviderPath
}

$target = 'x86_64-pc-windows-msvc'
Push-Location $root
try {
    cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'cargo fmt 检查失败。' }
    cargo clippy --all-targets --locked -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy 检查失败。' }
    cargo test --workspace --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo test 失败。' }
    $metadata = Get-CargoMetadata

    cargo --config 'target.x86_64-pc-windows-msvc.rustflags=["-C","target-feature=+crt-static"]' build --release --locked --target $target
    if ($LASTEXITCODE -ne 0) { throw 'cargo build --release 失败。' }

    $exe = Join-Path $metadata.target_directory "$target/release/nkyoa-tray.exe"
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw '发布目录缺少 nkyoa-tray.exe。'
    }

    [void] [IO.Directory]::CreateDirectory($outputLicenses)
    Copy-Item -LiteralPath $exe -Destination $output
    Copy-Item -LiteralPath (Join-Path $root 'THIRD_PARTY_NOTICES.md') -Destination $output
    $license = 'Slint-Royalty-free-2.0.md'
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "licenses/$license") -Destination $outputLicenses

    $zip = Join-Path $artifacts 'nkyoa-tray-rust-win-x64.zip'
    $payload = @(
        (Join-Path $output 'nkyoa-tray.exe')
        (Join-Path $output 'THIRD_PARTY_NOTICES.md')
        $outputLicenses
    )
    Compress-Archive -LiteralPath $payload -DestinationPath $zip -CompressionLevel Optimal -Force

    $exeSize = (Get-Item -LiteralPath $exe).Length
    $zipSize = (Get-Item -LiteralPath $zip).Length
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Output "exe bytes: $exeSize"
    Write-Output "zip bytes: $zipSize"
    Write-Output "zip sha256: $hash"
    Write-Output "package: $output"

    if ($Installer) {
        $version = ($metadata.packages | Where-Object { $_.name -eq 'nkyoa-tray' }).version
        & $IsccPath "/DAppVersion=$version" "/DPayloadDir=$output" "/O$artifacts" (Join-Path $PSScriptRoot 'NkyoaTray.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup 编译失败。' }
        $setup = Join-Path $artifacts 'nkyoa-tray-rust-win-x64-setup.exe'
        Write-Output "setup bytes: $((Get-Item -LiteralPath $setup).Length)"
        Write-Output "setup sha256: $((Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant())"
    }
}
finally {
    Pop-Location
}
