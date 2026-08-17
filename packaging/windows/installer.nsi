!ifndef VERSION
  !define VERSION "0.0.0"
!endif
Name "Classic Solitair"
OutFile "classic-solitair-${VERSION}-x86_64-setup.exe"
InstallDir "$PROGRAMFILES64\Classic Solitair"
RequestExecutionLevel admin
Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetOutPath "$INSTDIR"
  File "stage\classic-solitair.exe"
  File "stage\soltool.exe"
  File "stage\LICENSE"
  File "stage\README.md"
  CreateDirectory "$SMPROGRAMS\Classic Solitair"
  CreateShortcut "$SMPROGRAMS\Classic Solitair\Classic Solitair.lnk" "$INSTDIR\classic-solitair.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Classic Solitair\Uninstall.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\classic-solitair.exe"
  Delete "$INSTDIR\soltool.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Classic Solitair\Classic Solitair.lnk"
  Delete "$SMPROGRAMS\Classic Solitair\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Classic Solitair"
SectionEnd
