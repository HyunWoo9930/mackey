; MacKey installer — Inno Setup 6
; Build:  iscc installer\mackey.iss
; Expects the exe at ..\target\x86_64-pc-windows-gnu\release\mackey.exe
; (override with /DExePath=... )

#ifndef ExePath
#define ExePath "..\target\x86_64-pc-windows-gnu\release\mackey.exe"
#endif

[Setup]
AppId={{7B7C61E4-9D5B-4E1A-A54B-52F1D0A5C9A2}
AppName=MacKey
AppVersion=0.2.2
AppPublisher=MacKey
DefaultDirName={autopf}\MacKey
DisableProgramGroupPage=yes
DisableDirPage=yes
; the app itself requires admin (hooks must reach elevated windows)
PrivilegesRequired=admin
OutputBaseFilename=MacKeySetup
OutputDir=output
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
SetupIconFile=..\res\mackey.ico
UninstallDisplayIcon={app}\mackey.exe

; note: Korean.isl is an *unofficial* Inno language pack and is absent on a
; stock iscc install (CI) — default English UI is fine for a zero-config setup

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion

[Run]
; kill a previous instance if updating (ignore failures)
Filename: "{sys}\taskkill.exe"; Parameters: "/IM mackey.exe /F"; Flags: runhidden skipifdoesntexist; StatusMsg: "이전 인스턴스 종료 중..."
; register run-at-logon with highest privileges (Task Scheduler — a Run
; registry key cannot start an elevated program)
Filename: "{sys}\schtasks.exe"; Parameters: "/Create /F /TN MacKey /SC ONLOGON /RL HIGHEST /TR """"{app}\mackey.exe"""""; Flags: runhidden; StatusMsg: "자동 시작 등록 중..."
; zero-config: start immediately after install, no dialogs
Filename: "{app}\mackey.exe"; Flags: nowait; StatusMsg: "MacKey 시작 중..."

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM mackey.exe /F"; Flags: runhidden; RunOnceId: "KillMacKey"
Filename: "{sys}\schtasks.exe"; Parameters: "/Delete /F /TN MacKey"; Flags: runhidden; RunOnceId: "DelMacKeyTask"
