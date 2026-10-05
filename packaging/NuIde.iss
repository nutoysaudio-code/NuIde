; Instalador de NuIde (Inno Setup 6).
;
; Compilar (desde esta carpeta, con el binario release ya construido):
;   iscc packaging\NuIde.iss
;
; Instala la app y, opcionalmente, Node.js y las CLI de IA (opencode y las
; demás que se agreguen en postinstall.ps1).

#define AppName "NuIde"
#define AppVersion "0.1.0"
#define AppPublisher "NuToys"
#define AppURL "https://gitlab.com/nutoys-group/nuide"
#define AppExe "nuide.exe"

[Setup]
AppId={{6F0B2E3A-9C1D-4E2A-8B7C-3D5A1F0E9C42}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir=..\releases
OutputBaseFilename=NuIde-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\res\nuide.ico
UninstallDisplayIcon={app}\{#AppExe}
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Crear un acceso directo en el escritorio"; GroupDescription: "Accesos directos:"
Name: "associate"; Description: "Asociar los archivos .spasm con NuIde"; GroupDescription: "Integración:"; Flags: unchecked
Name: "nodejs"; Description: "Instalar Node.js (necesario para las CLI de IA)"; GroupDescription: "Dependencias:"; Flags: checkedonce
Name: "clis"; Description: "Instalar las CLI de IA (opencode y las demás)"; GroupDescription: "Dependencias:"; Flags: checkedonce

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "postinstall.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\.spasm"; ValueType: string; ValueName: ""; ValueData: "NuIde.Spasm"; Flags: uninsdeletevalue; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\NuIde.Spasm"; ValueType: string; ValueName: ""; ValueData: "Archivo SpASM"; Flags: uninsdeletekey; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\NuIde.Spasm\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: associate
Root: HKCU; Subkey: "Software\Classes\NuIde.Spasm\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: associate

[Run]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\postinstall.ps1"" -Clis"; StatusMsg: "Instalando Node.js y las CLI de IA..."; Flags: runhidden waituntilterminated; Tasks: clis
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\postinstall.ps1"""; StatusMsg: "Instalando Node.js..."; Flags: runhidden waituntilterminated; Tasks: nodejs and not clis
Filename: "{app}\{#AppExe}"; Description: "Ejecutar {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\docs"
