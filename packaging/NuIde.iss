; Instalador de NuIde (Inno Setup 6).
;
; Compilar (desde esta carpeta, con el binario release ya construido):
;   iscc packaging\NuIde.iss
;
; Instala la app y, opcionalmente, NuCoder (el agente de código: router de
; modelos gratis primero + coder).

#define AppName "NuIde"
#define AppVersion "0.3.2"
#define AppPublisher "NuToys"
#define AppURL "https://github.com/nutoysaudio-code/NuIde"
#define AppExe "nuide.exe"

[Setup]
AppId={{6F0B2E3A-9C1D-4E2A-8B7C-3D5A1F0E9C42}
AppName={#AppName}
AppVerName={#AppName} {#AppVersion}
AppVersion={#AppVersion}
VersionInfoVersion=0.3.2.0
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
Name: "path"; Description: "Agregar NuIde, NuCoder y NuCook a la variable PATH"; GroupDescription: "Integración:"; Flags: checkedonce
Name: "nucoder"; Description: "Instalar NuCoder (agente) y NuCook (cocina de skills)"; GroupDescription: "Componentes:"; Flags: checkedonce

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\nucoder\target\release\nucoder.exe"; DestDir: "{app}"; Flags: ignoreversion; Tasks: nucoder
Source: "..\nucoder\target\release\nucook.exe"; DestDir: "{app}"; Flags: ignoreversion; Tasks: nucoder
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
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\postinstall.ps1"" -AddPath"; StatusMsg: "Agregando NuIde, NuCoder y NuCook al PATH..."; Flags: runhidden waituntilterminated; Tasks: path
Filename: "{app}\{#AppExe}"; Description: "Ejecutar {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\docs"
