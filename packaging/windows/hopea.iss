; SPDX-FileCopyrightText: 2026 Daniil Klimov
; SPDX-License-Identifier: GPL-3.0-or-later
; Compile through build.ps1, which validates inputs and supplies the complete bundle.
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef BundleDir
  #error BundleDir is required
#endif
#ifndef OutputPath
  #error OutputPath is required
#endif
#ifndef IconPath
  #error IconPath is required
#endif
#ifndef Repository
  #error Repository is required
#endif

[Setup]
AppId=io.hopea.Hopea
AppName=Hopea
AppVersion={#AppVersion}
AppPublisher=Hopea contributors
AppPublisherURL=https://github.com/{#Repository}
AppSupportURL=https://github.com/{#Repository}/issues
AppUpdatesURL=https://github.com/{#Repository}/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Hopea
PrivilegesRequired=lowest
UsePreviousAppDir=yes
UsePreviousTasks=yes
DisableProgramGroupPage=yes
DisableWelcomePage=yes
DisableDirPage=auto
WizardStyle=modern dynamic
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; The shipped Rust/UCRT64/Python runtime requires Windows 10, regardless of
; whether an older OS can execute the installer itself.
MinVersion=10.0
OutputDir={#OutputPath}
OutputBaseFilename=hopea-{#AppVersion}-windows-x86_64-setup
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\hopea.ico
Compression=lzma2/normal
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
SetupLogging=yes
UninstallLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#IconPath}"; DestDir: "{app}"; DestName: "hopea.ico"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Hopea"; Filename: "{app}\bin\hopea.exe"; WorkingDir: "{app}"; IconFilename: "{app}\hopea.ico"; AppUserModelID: "io.hopea.Hopea"
Name: "{userdesktop}\Hopea"; Filename: "{app}\bin\hopea.exe"; WorkingDir: "{app}"; IconFilename: "{app}\hopea.ico"; Tasks: desktopicon; AppUserModelID: "io.hopea.Hopea"

[Run]
Filename: "{app}\bin\hopea.exe"; Description: "{cm:LaunchProgram,Hopea}"; Flags: nowait postinstall skipifsilent

; No recursive UninstallDelete: remove only installer-owned files. Music,
; library databases, credentials and settings live outside the install tree.
