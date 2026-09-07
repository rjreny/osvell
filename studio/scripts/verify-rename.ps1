$ErrorActionPreference = 'Stop'
$fixture = Join-Path $PSScriptRoot '../.release-check/rename'
New-Item -ItemType Directory -Force -Path $fixture | Out-Null
$registryRoot = 'Software\OsvellRenameTest-' + [guid]::NewGuid().ToString('N')
$hooks = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot '../src-tauri/installer-hooks.nsh')
$hooks = $hooks.Replace('Software\rjreny', "$registryRoot\rjreny").Replace('Software\Microsoft\Windows\CurrentVersion\Uninstall', "$registryRoot\Uninstall")
$hooks = $hooks.Replace('$SMPROGRAMS', '$EXEDIR\start-menu').Replace('$DESKTOP', '$EXEDIR\desktop')
Set-Content -LiteralPath (Join-Path $fixture 'hooks.nsh') -Value $hooks
$nsi = @'
Unicode true
RequestExecutionLevel user
SilentInstall silent
OutFile "fixture.exe"
!include MUI2.nsh
!include FileFunc.nsh
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "@UTILS@"
!addplugindir "@PLUGINS@"
!define MAINBINARYNAME "osvell-rename-fixture-never-running"
!include "hooks.nsh"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!macro AssertEqual ACTUAL EXPECTED LABEL
  ${If} "${ACTUAL}" != "${EXPECTED}"
    FileOpen $9 "$EXEDIR\result.txt" w
    FileWrite $9 "FAIL: ${LABEL}"
    FileClose $9
    SetErrorLevel 1
    Quit
  ${EndIf}
!macroend

Section
  StrCpy $INSTDIR "$EXEDIR\fresh"
  Call OsvellUseExistingInstall
  !insertmacro AssertEqual $INSTDIR "$EXEDIR\fresh" "fresh install path"
  !insertmacro AssertEqual $OsvellLegacyInstall "" "fresh install not migrated"

  CreateDirectory "$EXEDIR\legacy"
  FileOpen $9 "$EXEDIR\legacy\studio.exe" w
  FileWrite $9 "fixture"
  FileClose $9
  WriteRegStr HKCU "@REG@\rjreny\Studio" "" "$EXEDIR\legacy"
  WriteRegStr HKCU "@REG@\Uninstall\Studio" "Publisher" "someone-else"
  Call OsvellUseExistingInstall
  !insertmacro AssertEqual $INSTDIR "$EXEDIR\fresh" "other publisher left alone"

  WriteRegStr HKCU "@REG@\Uninstall\Studio" "Publisher" "rjreny"
  WriteRegStr HKCU "@REG@\rjreny\Studio" "" "$EXEDIR\missing"
  Call OsvellUseExistingInstall
  !insertmacro AssertEqual $INSTDIR "$EXEDIR\fresh" "missing binary left alone"

  WriteRegStr HKCU "@REG@\rjreny\Studio" "" "$EXEDIR\legacy"
  CreateDirectory "$EXEDIR\start-menu"
  CreateDirectory "$EXEDIR\desktop"
  CreateShortcut "$EXEDIR\start-menu\Studio.lnk" "$EXEDIR\legacy\${MAINBINARYNAME}.exe"
  CreateShortcut "$EXEDIR\desktop\Studio.lnk" "$EXEDIR\other-app.exe"
  FileOpen $9 "$EXEDIR\legacy\studio.db" w
  FileWrite $9 "history-must-survive"
  FileClose $9
  !insertmacro NSIS_HOOK_PREINSTALL
  !insertmacro AssertEqual $INSTDIR "$EXEDIR\legacy" "silent upgrade reuses legacy directory"
  !insertmacro AssertEqual $OUTDIR "$EXEDIR\legacy" "file extraction directory updated"
  ; Simulate the registry writes performed by the standard installer.
  WriteRegStr HKCU "@REG@\rjreny\Osvell" "" $INSTDIR
  WriteRegStr HKCU "@REG@\Uninstall\Osvell" "DisplayName" "Osvell"
  !insertmacro NSIS_HOOK_POSTINSTALL
  ReadRegStr $R0 HKCU "@REG@\Uninstall\Studio" "Publisher"
  !insertmacro AssertEqual $R0 "" "legacy registration removed"
  ReadRegStr $R0 HKCU "@REG@\Uninstall\Osvell" "DisplayName"
  !insertmacro AssertEqual $R0 "Osvell" "new registration preserved"
  FileOpen $9 "$EXEDIR\legacy\studio.db" r
  FileRead $9 $R0
  FileClose $9
  !insertmacro AssertEqual $R0 "history-must-survive" "history preserved"
  !insertmacro IsShortcutTarget "$EXEDIR\start-menu\Osvell.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  Pop $R0
  !insertmacro AssertEqual $R0 1 "matching shortcut renamed with target intact"
  !insertmacro IsShortcutTarget "$EXEDIR\desktop\Studio.lnk" "$EXEDIR\other-app.exe"
  Pop $R0
  !insertmacro AssertEqual $R0 1 "unrelated shortcut left alone"

  StrCpy $OsvellLegacyInstall ""
  StrCpy $INSTDIR "$EXEDIR\legacy"
  WriteRegStr HKCU "@REG@\rjreny\Studio" "" "$EXEDIR\other"
  Call OsvellUseExistingInstall
  !insertmacro AssertEqual $INSTDIR "$EXEDIR\legacy" "repeat Osvell upgrade path stable"
  !insertmacro AssertEqual $OsvellLegacyInstall "" "repeat upgrade not remigrated"
  DeleteRegKey HKCU "@REG@"
  FileOpen $9 "$EXEDIR\result.txt" w
  FileWrite $9 "PASS: fresh, publisher guard, missing binary, silent upgrade, output path, registry migration, shortcut targeting, data preservation, repeat upgrade"
  FileClose $9
SectionEnd
'@
Set-Content -LiteralPath (Join-Path $fixture 'fixture.nsi') -Value $nsi.Replace('@REG@', $registryRoot).Replace('@UTILS@', (Join-Path $PSScriptRoot '../src-tauri/target/release/nsis/x64/utils.nsh')).Replace('@PLUGINS@', (Join-Path $env:LOCALAPPDATA 'tauri/NSIS/Plugins/x86-unicode/additional'))
Push-Location $fixture
try {
  & (Join-Path $env:LOCALAPPDATA 'tauri/NSIS/makensis.exe') /V2 fixture.nsi
  if ($LASTEXITCODE -ne 0) { throw 'NSIS fixture failed to compile' }
  $fixtureProcess = Start-Process -FilePath (Join-Path $fixture 'fixture.exe') -WindowStyle Hidden -Wait -PassThru
  Get-Content -LiteralPath (Join-Path $fixture 'result.txt')
  if ($fixtureProcess.ExitCode -ne 0) { throw 'NSIS migration fixture failed' }
} finally {
  Pop-Location
  # A unique fixture-only key, never the installed app's registry entries.
  Remove-Item -LiteralPath "HKCU:\$registryRoot" -Recurse -Force -ErrorAction SilentlyContinue
}

