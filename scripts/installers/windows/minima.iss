; Inno Setup script for the graphical Windows installer.
; Built by scripts\installers\windows\build-setup.bat, which passes:
;   /DAppVersion=0.1.0 /DSourceExe=C:\...\minima.exe /DArch=x86_64 /DOutDir=C:\...\dist /DRepoRoot=C:\...\minima
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Arch
  #define Arch "x86_64"
#endif

[Setup]
AppId={{6F1E3B2A-5C0D-4B8E-9A47-3E5D2C1B7A90}
AppName=Minima
AppVersion={#AppVersion}
AppPublisher=Minima
AppPublisherURL=https://github.com/
DefaultDirName={autopf}\Minima
DefaultGroupName=Minima
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutDir}
OutputBaseFilename=minima-{#AppVersion}-windows-{#Arch}-setup
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
UninstallDisplayName=Minima
LicenseFile={#RepoRoot}\LICENSE
SetupIconFile={#RepoRoot}\assets\minima.ico
UninstallDisplayIcon={app}\minima.exe
#if Arch == "aarch64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add Minima to PATH (so you can type ""minima"" in any terminal)"; GroupDescription: "Integration:"
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Integration:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "minima.exe"; Flags: ignoreversion
Source: "{#RepoRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Minima"; Filename: "{app}\minima.exe"; WorkingDir: "{%USERPROFILE}"
Name: "{group}\Uninstall Minima"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Minima"; Filename: "{app}\minima.exe"; WorkingDir: "{%USERPROFILE}"; Tasks: desktopicon

[Registry]
; per-user PATH when installed without admin, machine PATH when elevated
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsPathUser
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsPathMachine

[Run]
Filename: "{app}\minima.exe"; Description: "Launch Minima"; Flags: nowait postinstall skipifsilent unchecked shellexec

[Code]
function PathMissing(Root: Integer; SubKey: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(Root, SubKey, 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(ExpandConstant('{app}')) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

function NeedsPathUser: Boolean;
begin
  Result := (not IsAdminInstallMode) and PathMissing(HKCU, 'Environment');
end;

function NeedsPathMachine: Boolean;
begin
  Result := IsAdminInstallMode and PathMissing(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment');
end;

procedure RemovePathEntry(Root: Integer; SubKey: string);
var
  P, Entry: string;
  I: Integer;
begin
  if not RegQueryStringValue(Root, SubKey, 'Path', P) then exit;
  Entry := ExpandConstant('{app}');
  I := Pos(';' + Uppercase(Entry), Uppercase(P));
  if I > 0 then
  begin
    Delete(P, I, Length(Entry) + 1);
    RegWriteExpandStringValue(Root, SubKey, 'Path', P);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RemovePathEntry(HKCU, 'Environment');
    if IsAdminInstallMode then
      RemovePathEntry(HKLM, 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment');
  end;
end;
