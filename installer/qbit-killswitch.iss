; qbit-killswitch — Inno Setup installer script
;
; Build requirements:
;   - Inno Setup 6.x  (https://jrsoftware.org/isinfo.php)
;   - Run `cargo build --release` first so the binary exists at
;     ..\target\release\qbit-killswitch.exe
;
; Usage:
;   iscc installer\qbit-killswitch.iss
;   iscc /DAppVersion=0.4.0 installer\qbit-killswitch.iss
;
; Output: installer\output\qbit-killswitch-setup-{version}.exe

#ifndef AppVersion
  #define AppVersion "0.4.0"
#endif

#define AppName      "qbit-killswitch"
#define AppPublisher "Londopy"
#define AppURL       "https://github.com/Londopy/qbit-killswitch"
#define AppExeName   "qbit-killswitch.exe"
#define AppRegKey    "qbit-killswitch"   ; matches auto-launch registry value name

[Setup]
AppId={{A3F1C2D4-8B7E-4F3A-9C1D-2E5F6A7B8C9D}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases

; Install to Program Files without requiring elevation
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
AllowNoIcons=yes

LicenseFile=..\LICENSE

OutputDir=output
OutputBaseFilename=qbit-killswitch-setup-{#AppVersion}

; Compression
Compression=lzma2/ultra64
SolidCompression=yes

; Windows 10+ only (64-bit)
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

WizardStyle=modern
WizardSizePercent=120

; Suppress console on launch
WindowVisible=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; \
  Description: "{cm:CreateDesktopIcon}"; \
  GroupDescription: "{cm:AdditionalIcons}"

[Files]
; Main binary — built by `cargo build --release`
Source: "..\target\release\{#AppExeName}"; \
  DestDir: "{app}"; \
  Flags: ignoreversion

[Icons]
; Start Menu
Name: "{group}\{#AppName}";                    Filename: "{app}\{#AppExeName}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"

; Desktop (optional task)
Name: "{autodesktop}\{#AppName}"; \
  Filename: "{app}\{#AppExeName}"; \
  Tasks: desktopicon

[Run]
; Offer to launch after install
Filename: "{app}\{#AppExeName}"; \
  Description: "{cm:LaunchProgram,{#AppName}}"; \
  Flags: nowait postinstall skipifsilent

[UninstallRun]
; Stop the running process gracefully before files are removed
Filename: "taskkill.exe"; \
  Parameters: "/IM {#AppExeName} /F"; \
  Flags: runhidden; \
  RunOnceId: "KillProcess"

[Registry]
; Remove the auto-launch Run entry if the user had it enabled
; (the app writes this key itself; we clean it up on uninstall)
Root: HKCU; \
  Subkey: "SOFTWARE\Microsoft\Windows\CurrentVersion\Run"; \
  ValueName: "{#AppRegKey}"; \
  Flags: deletevalue uninsdeletevalue; \
  Check: RegValueExists(HKCU, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Run', '{#AppRegKey}')

[Code]
// Show a reminder on the finish page if the Web UI note is relevant.
procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpFinished then
    WizardForm.FinishedLabel.Caption :=
      WizardForm.FinishedLabel.Caption + #13#10#13#10 +
      'Reminder: make sure qBittorrent''s Web UI is enabled' + #13#10 +
      '(qBittorrent → Tools → Options → Web UI → Enable the Web User Interface).';
end;
