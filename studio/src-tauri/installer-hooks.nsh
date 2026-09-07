; Keep the legacy executable path and identifier for data and taskbar compatibility.
Var OsvellLegacyInstall

; Before interactive pages; PREINSTALL repeats this for silent updater installs.
!define MUI_CUSTOMFUNCTION_GUIINIT OsvellUseExistingInstall
Function OsvellUseExistingInstall
  Push $R0
  Push $R1
  ReadRegStr $R0 HKCU "Software\rjreny\Osvell" ""
  ${If} $R0 == ""
    ReadRegStr $R0 HKCU "Software\rjreny\Studio" ""
    ReadRegStr $R1 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Studio" "Publisher"
    ${If} $R0 != ""
    ${AndIf} $R1 == "rjreny"
    ${AndIf} ${FileExists} "$R0\studio.exe"
      StrCpy $OsvellLegacyInstall $R0
      StrCpy $INSTDIR $R0
    ${EndIf}
  ${EndIf}
  Pop $R1
  Pop $R0
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  Call OsvellUseExistingInstall
  ; SetOutPath ran before this hook; update it after selecting the legacy path.
  SetOutPath $INSTDIR
  nsis_tauri_utils::KillProcessCurrentUser "${MAINBINARYNAME}.exe"
  Pop $R0
  Sleep 500
!macroend

!macro OsvellRenameShortcut DIRECTORY
  ; Touch only shortcuts targeting this installation.
  !insertmacro IsShortcutTarget "${DIRECTORY}\Studio.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $R0
  ${If} $R0 == 1
    ${If} ${FileExists} "${DIRECTORY}\Osvell.lnk"
      Delete "${DIRECTORY}\Studio.lnk"
    ${Else}
      Rename "${DIRECTORY}\Studio.lnk" "${DIRECTORY}\Osvell.lnk"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ${If} $OsvellLegacyInstall != ""
  ${AndIf} $OsvellLegacyInstall == $INSTDIR
    !insertmacro OsvellRenameShortcut "$SMPROGRAMS"
    !insertmacro OsvellRenameShortcut "$DESKTOP"
    ; The new uninstaller and registry entry have now been written.
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Studio"
    DeleteRegKey HKCU "Software\rjreny\Studio"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsis_tauri_utils::KillProcessCurrentUser "${MAINBINARYNAME}.exe"
  Pop $R0
  Sleep 500
!macroend
