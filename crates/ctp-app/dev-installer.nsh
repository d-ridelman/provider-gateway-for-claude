; dev-сборка: имена ярлыков и папка в меню «Пуск» отличаются от обычной установки,
; чтобы установленное приложение и dev-копия жили рядом и не путались.
!macro NSIS_HOOK_PREINSTALL
  Delete "$SMPROGRAMS\Provider Gateway for Claude.lnk"
  Delete "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  CreateShortcut "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk" "$INSTDIR\provider-gateway.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$SMPROGRAMS\Provider Gateway for Claude (dev).lnk"
!macroend
