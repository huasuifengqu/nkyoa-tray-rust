#define AppName "OA通知"
#define AppExe "nkyoa-tray.exe"
#define Aumid "NKYOA.Tray.Rust.DesktopNotifications"
#define Protocol "nkyoa-notify-rust"

[Setup]
AppId={{75C3C10F-B930-4D1E-8380-0CB7A59CC819}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Nkyoa.Tray.Rust
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=yes
PrivilegesRequired=lowest
MinVersion=10.0.22000
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
UninstallDisplayIcon={app}\{#AppExe}
RestartApplications=no
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
OutputBaseFilename=nkyoa-tray-rust-win-x64-setup
VersionInfoVersion={#AppVersion}

[Languages]
Name: "zhcn"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Files]
Source: "{#PayloadDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\licenses\Slint-Royalty-free-2.0.md"; DestDir: "{app}\licenses"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; AppUserModelID: "{#Aumid}"; AppUserModelToastActivatorCLSID: "E82D235E-46BC-4018-A883-65AEBDE7E1BA"

[InstallDelete]
; 改名前的旧快捷方式，以及脚本安装方式留下的卸载脚本，避免残留或双入口
Type: files; Name: "{userprograms}\nkyoa 桌面通知 (Rust).lnk"
Type: files; Name: "{app}\Install-NkyoaTray.ps1"

[Registry]
; Install-NkyoaTray.ps1 会在同一位置注册自己的卸载项；由 Inno 接管时必须移除，
; 否则「设置 → 应用」会同时出现两个入口，其中一个指向已被覆盖的 unins000.exe。
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Uninstall\NKYOA.Tray.Rust"; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\{#Aumid}"; ValueType: string; ValueName: "DisplayName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueData: "URL:{#AppName}"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\{#Protocol}"; ValueType: string; ValueName: "URL Protocol"
Root: HKCU; Subkey: "Software\Classes\{#Protocol}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "NKYOA.Tray.Rust"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "打开 OA通知"; Flags: postinstall nowait skipifsilent

[Code]
const
  ClientRunningMessage = '客户端正在运行。请从系统托盘退出客户端后重试。';

function ClientIsRunning: Boolean;
var
  ClientPath: String;
  ExitCode: Integer;
begin
  Result := False;
  ClientPath := ExpandConstant('{app}\{#AppExe}');
  if FileExists(ClientPath) then
    if Exec(ClientPath, '--check-instance', '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then
      Result := ExitCode = 3;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if ClientIsRunning then
    Result := ClientRunningMessage;
end;

function InitializeUninstall: Boolean;
begin
  Result := not ClientIsRunning;
  if not Result then
    SuppressibleMsgBox(ClientRunningMessage, mbError, MB_OK, IDOK);
end;
