; DSHL Windows installer (NSIS 3).
;
; Build from the repository root after staging `dshl.exe` + `dshl.toml` into a
; directory (default `stage`):
;
;   makensis -V3 -DSTAGE_DIR=stage -DPRODUCT_VERSION=0.1.0 \
;            -DOUTFILE=dshl-setup.exe packing/windows/dshl.nsi
;
; NOTE: some makensis builds resolve `File`/`Icon` paths relative to the
; *script's* directory rather than the working directory, so pass STAGE_DIR and
; OUTFILE as ABSOLUTE paths (e.g. `-DSTAGE_DIR=$GITHUB_WORKSPACE/stage` in CI).
;
; The script must stay UTF-8 with BOM (the leading U+FEFF) so makensis parses
; the SimpChinese strings correctly. Edit with the write tool (full rewrite);
; plain edits strip the BOM and break compilation.

Unicode true
!include "MUI2.nsh"

!ifndef STAGE_DIR
  !define STAGE_DIR "."
!endif
!ifndef PRODUCT_VERSION
  !define PRODUCT_VERSION "0.0.0"
!endif
!ifndef OUTFILE
  !define OUTFILE "dshl-setup.exe"
!endif

; ---------------------------------------------------------------------------
; Naming: the user-visible product names and the installed binary are kept
; intentionally different. Shortcuts / Start Menu / Add-Remove Programs show
; APP_NAME (or APP_FULL_NAME); the executable stays BINARY_NAME (dshl.exe).
; This is a third-party launcher for the DeepSeek Harness, not an official
; DeepSeek product, so no DeepSeek company/copyright metadata is claimed.
; ---------------------------------------------------------------------------
!define APP_NAME       "DSHL"
!define APP_FULL_NAME  "DeepSeek Harness Launcher"
!define BINARY_NAME    "dshl.exe"

Name "${APP_NAME} — ${APP_FULL_NAME}"
OutFile "${OUTFILE}"
; Per-user install: default %LOCALAPPDATA%\Programs\dshl (no admin needed),
; freely changeable on the MUI_PAGE_DIRECTORY screen; the last choice is
; remembered in HKCU\Software\dshl.
InstallDir "$LOCALAPPDATA\Programs\dshl"
InstallDirRegKey HKCU "Software\dshl" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!insertmacro MUI_PAGE_WELCOME
; Optional components (desktop shortcut) come before the directory picker.
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${BINARY_NAME}"
!define MUI_FINISHPAGE_RUN_TEXT "$(RunDSHL)"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"
LangString RunDSHL ${LANG_ENGLISH} "Run ${APP_NAME}"
LangString RunDSHL ${LANG_SIMPCHINESE} "运行 ${APP_NAME}"
LangString SecDesktop ${LANG_ENGLISH} "Desktop shortcut"
LangString SecDesktop ${LANG_SIMPCHINESE} "桌面快捷方式"

VIProductVersion "${PRODUCT_VERSION}.0"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "FileDescription" "${APP_FULL_NAME}"
VIAddVersionKey "FileVersion" "${PRODUCT_VERSION}"
VIAddVersionKey "ProductVersion" "${PRODUCT_VERSION}"

; Core application files (always installed).
Section "${APP_NAME}" SEC_MAIN
  SectionIn RO
  SetOutPath "$INSTDIR"

  File "${STAGE_DIR}\${BINARY_NAME}"
  File "${STAGE_DIR}\dshl.toml"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Start Menu registration.
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${BINARY_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"

  ; Add/Remove Programs entry (per-user).
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "DisplayName" "${APP_NAME} — ${APP_FULL_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "Publisher" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "DisplayIcon" "$INSTDIR\${BINARY_NAME},0"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl" \
    "NoRepair" 1
  WriteRegStr HKCU "Software\dshl" "InstallDir" "$INSTDIR"
SectionEnd

; Optional desktop shortcut (checked by default, uncheckable on the
; components page).
Section "$(SecDesktop)" SEC_DESKTOP
  SetOutPath "$INSTDIR"
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${BINARY_NAME}"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\${BINARY_NAME}"
  Delete "$INSTDIR\dshl.toml"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dshl"
  DeleteRegKey HKCU "Software\dshl"
SectionEnd

; Installer/uninstaller icons (from the staged packing icon).
Icon "${STAGE_DIR}\dsh.ico"
UninstallIcon "${STAGE_DIR}\dsh.ico"
