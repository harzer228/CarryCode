#ifndef AppVersion
  #error Pass /DAppVersion=<release version>
#endif
#ifndef BundleDir
  #error Pass /DBundleDir=<extracted release ZIP>
#endif

[Setup]
AppId=flashGPT.Portable
AppName=CarryCode
AppVersion={#AppVersion}
AppPublisher=CarryCode contributors
AppPublisherURL=https://github.com/harzer228/flashGPT
DefaultDirName={localappdata}\Programs\flashGPT
DisableDirPage=no
DisableProgramGroupPage=yes
AppendDefaultDirName=no
DirExistsWarning=yes
PrivilegesRequired=lowest
Uninstallable=no
CloseApplications=no
ArchitecturesAllowed=x64os
SetupArchitecture=x64
MinVersion=10.0.22000
SetupIconFile=..\assets\flashgpt.ico
WizardStyle=modern
Compression=lzma2/fast
OutputBaseFilename=flashGPT-setup-windows-x64

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Messages]
english.SelectDirDesc=Choose a folder on this computer or on your USB drive.
english.SelectDirLabel3=For a new USB setup, choose a folder such as F:\CarryCode. To update flashGPT or CarryCode, close the launcher and its CLI, then select your existing folder.
russian.SelectDirDesc=Выбери папку на компьютере или на флешке.
russian.SelectDirLabel3=Для новой установки на флешку выбери папку, например F:\CarryCode. Для обновления flashGPT или CarryCode закрой лаунчер и CLI, затем выбери существующую папку.

[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Run]
Filename: "{app}\flashGPT.exe"; WorkingDir: "{app}"; Description: "{cm:LaunchProgram,CarryCode}"; Flags: postinstall nowait skipifsilent unchecked
