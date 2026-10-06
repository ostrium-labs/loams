; Loams Desktop for Windows — per-user installer (Inno Setup 6).
;
; Built by scripts/package-windows.ps1, which passes the version, the package
; architecture, and the staged portable directory:
;   ISCC.exe /DAppVersion=0.2.97 /DArch=x86_64 /DPackageDir=<stage> /DOutputDir=<out> loams-desktop.iss
;
; Installs into %LOCALAPPDATA%\Programs\Loams Desktop without elevation, like VS
; Code's user setup: the directory stays writable by its user, so the in-app
; updater (crates/update/src/windows.rs) can replace loams-desktop.exe in place. The
; staged directory already carries loams-desktop-update.json, which marks the install
; as update-managed. Re-running a newer installer upgrades in place; user data
; lives in %LOCALAPPDATA%\Loams Desktop and is never touched here.

#ifndef AppVersion
  #error AppVersion must be defined (/DAppVersion=x.y.z)
#endif
#ifndef Arch
  #error Arch must be defined (/DArch=x86_64 or /DArch=aarch64)
#endif
#ifndef PackageDir
  #error PackageDir must be defined (/DPackageDir=<staged package directory>)
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#if Arch == "aarch64"
  #define ArchAllowed "arm64"
#else
  #define ArchAllowed "x64compatible"
#endif

[Setup]
; Never change AppId: it identifies the installation across upgrades, and
; crates/update/src/windows.rs refreshes DisplayVersion under this key after
; in-app updates.
AppId={{93DB7E9E-5B92-5E45-99A1-105C32A995B8}
AppName=Loams Desktop
AppVersion={#AppVersion}
AppVerName=Loams Desktop {#AppVersion}
AppPublisher=Loams Desktop
AppPublisherURL=https://loams.dev
AppSupportURL=https://loams.dev
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Loams Desktop
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=loams-desktop-{#AppVersion}-windows-{#Arch}-setup
SetupIconFile=loams-desktop.ico
UninstallDisplayIcon={app}\loams-desktop.exe
UninstallDisplayName=Loams Desktop
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; A running Loams Desktop is closed through the Restart Manager before its files are
; replaced; the updated app starts again from the finish page.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\loams-desktop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\SCOPED_NOTICE.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Loams Desktop"; Filename: "{app}\loams-desktop.exe"
Name: "{autodesktop}\Loams Desktop"; Filename: "{app}\loams-desktop.exe"; Tasks: desktopicon

[Registry]
; loams:// conversation links — the scheme macOS registers in Info.plist and
; Linux in loams-desktop.desktop.
Root: HKCU; Subkey: "Software\Classes\loams"; ValueType: string; ValueName: ""; ValueData: "URL:Loams Desktop"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\loams"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\loams\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\loams-desktop.exe"",0"
Root: HKCU; Subkey: "Software\Classes\loams\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\loams-desktop.exe"" ""%1"""

[Run]
Filename: "{app}\loams-desktop.exe"; Description: "{cm:LaunchProgram,Loams Desktop}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Leftovers of in-app updates (crates/update/src/windows.rs).
Type: files; Name: "{app}\loams-desktop.exe.old"
Type: files; Name: "{app}\.loams-desktop-update-incoming.exe"
Type: filesandordirs; Name: "{app}\.loams-desktop-update-*"
