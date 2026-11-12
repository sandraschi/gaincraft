; GainCraft per-user NSIS installer (no admin rights, no UAC prompt).
; Installs to %LOCALAPPDATA%\Programs\GainCraft. Built by scripts/release.ps1:
;   makensis /DPRODUCT_VERSION=x.y.z /DOUT_FILE=dist\gaincraft-x.y.z-setup.exe installer\installer.nsi
; The database is NOT in the install folder (%LOCALAPPDATA%\gaincraft\gaincraft.db),
; so uninstalling never deletes customer data.

!ifndef PRODUCT_VERSION
  !error "pass /DPRODUCT_VERSION=x.y.z"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\gaincraft-${PRODUCT_VERSION}-setup.exe"
!endif

!define PRODUCT_NAME "GainCraft"
!define PRODUCT_PUBLISHER "sandraschi"
!define PRODUCT_WEB_SITE "https://github.com/sandraschi/gaincraft"
!define PRODUCT_DIR_REGKEY "Software\Microsoft\Windows\CurrentVersion\App Paths\gaincraft-app.exe"
!define PRODUCT_UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"

Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\GainCraft"
InstallDirRegKey HKCU "${PRODUCT_DIR_REGKEY}" ""
ShowInstDetails show
ShowUnInstDetails show

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "GainCraft" SEC01
  SetOutPath "$INSTDIR"
  SetOverwrite on
  ; Missing binaries are an error: a half-empty installer is worse than none.
  File "..\target\release\gaincraft-app.exe"
  File "..\target\release\gaincraft.exe"
  File "..\README.md"
  File "..\HELP.md"
  File "..\CHANGELOG.md"

  CreateDirectory "$SMPROGRAMS\GainCraft"
  CreateShortCut "$SMPROGRAMS\GainCraft\GainCraft.lnk" "$INSTDIR\gaincraft-app.exe"
  CreateShortCut "$SMPROGRAMS\GainCraft\GainCraft Help.lnk" "$INSTDIR\HELP.md"
  CreateShortCut "$SMPROGRAMS\GainCraft\Uninstall GainCraft.lnk" "$INSTDIR\uninst.exe"

  WriteUninstaller "$INSTDIR\uninst.exe"
  WriteRegStr HKCU "${PRODUCT_DIR_REGKEY}" "" "$INSTDIR\gaincraft-app.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayName" "$(^Name)"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "UninstallString" "$INSTDIR\uninst.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayIcon" "$INSTDIR\gaincraft-app.exe"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "URLInfoAbout" "${PRODUCT_WEB_SITE}"
  WriteRegStr HKCU "${PRODUCT_UNINST_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegDWORD HKCU "${PRODUCT_UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${PRODUCT_UNINST_KEY}" "NoRepair" 1
SectionEnd

Section Uninstall
  Delete "$SMPROGRAMS\GainCraft\GainCraft.lnk"
  Delete "$SMPROGRAMS\GainCraft\GainCraft Help.lnk"
  Delete "$SMPROGRAMS\GainCraft\Uninstall GainCraft.lnk"
  RMDir "$SMPROGRAMS\GainCraft"
  Delete "$INSTDIR\gaincraft-app.exe"
  Delete "$INSTDIR\gaincraft.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\HELP.md"
  Delete "$INSTDIR\CHANGELOG.md"
  Delete "$INSTDIR\uninst.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${PRODUCT_UNINST_KEY}"
  DeleteRegKey HKCU "${PRODUCT_DIR_REGKEY}"
  SetAutoClose true
SectionEnd
