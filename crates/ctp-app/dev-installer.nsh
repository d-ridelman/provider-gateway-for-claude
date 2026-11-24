; dev-сборка: имена ярлыков и папка в меню «Пуск» отличаются от обычной установки,
; чтобы установленное приложение и dev-копия жили рядом и не путались.
!macro NSIS_HOOK_PREINSTALL
  Delete "$SMPROGRAMS\Provider Gateway for Claude.lnk"
  Delete "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk" "$INSTDIR\provider-gateway.exe"
  ; Как в installer.nsh: Explorer перечитывает значок exe и ярлыка, а не берёт старый из кеша.
  System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x5, w "$INSTDIR\provider-gateway.exe", p 0)'
  System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x5, w "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk", p 0)'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk"
!macroend
