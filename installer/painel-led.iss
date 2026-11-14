; Instalador do Painel LED (Inno Setup 6).
; Compilar a partir da raiz do projeto, depois de `cargo build --release`:
;   ISCC.exe /DAppVersion=0.1.0 /DNumVersion=0.1.0.0 installer\painel-led.iss
; O resultado vai para dist\.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef NumVersion
  #define NumVersion "0.0.0.0"
#endif

[Setup]
AppId={{6F1C2B7E-4E0A-4C51-9C7B-2D5A8E3F1A42}
AppName=Painel LED
AppVersion={#AppVersion}
AppVerName=Painel LED {#AppVersion}
AppPublisher=Sérgio Berlotto Jr
AppPublisherURL=https://github.com/berlotto/painel-led-auxiliar
AppSupportURL=https://github.com/berlotto/painel-led-auxiliar/issues
VersionInfoVersion={#NumVersion}
; Mesma pasta usada pelo install.ps1; instala só para o usuário, sem pedir administrador.
DefaultDirName={localappdata}\Programs\painel-led
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=painel-led-{#AppVersion}-instalador
SetupIconFile=..\assets\painel-led.ico
UninstallDisplayIcon={app}\painel-led.exe
UninstallDisplayName=Painel LED
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=force

[Languages]
Name: "ptbr"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "autostart"; Description: "Iniciar o painel junto com o Windows"
Name: "desktopicon"; Description: "Criar atalho na área de trabalho"; Flags: unchecked

[Files]
Source: "..\target\release\painel-led.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\painel-led-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Painel LED"; Filename: "{app}\painel-led.exe"
Name: "{userdesktop}\Painel LED"; Filename: "{app}\painel-led.exe"; Tasks: desktopicon

[Registry]
; Mesmo valor que o painel grava ao ligar "Iniciar com o Windows" nas configurações.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PainelLED"; ValueData: """{app}\painel-led.exe"""; Tasks: autostart

[Run]
Filename: "{app}\painel-led.exe"; Description: "Abrir o Painel LED agora"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopPanel();
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM painel-led.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM painel-led-mcp.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopPanel();
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    StopPanel();
    // O valor pode ter sido criado pelo painel (configurações), não só pelo instalador.
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'PainelLED');
  end;
end;
