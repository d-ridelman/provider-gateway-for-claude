; Hooks of the release installer (tauri.conf.json bundle.windows.nsis.installerHooks).
; After an update the exe keeps its path, and Explorer keeps drawing the old icon for it and
; its shortcuts (taskbar, Start, desktop) from its icon cache. Telling the shell these files
; changed makes it read the new icon now, not after a reboot.
!macro NSIS_HOOK_POSTINSTALL
  System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x5, w "$INSTDIR\provider-gateway.exe", p 0)'
  System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x5, w "$SMPROGRAMS\Provider Gateway for Claude.lnk", p 0)'
  System::Call 'shell32::SHChangeNotify(i 0x2000, i 0x5, w "$DESKTOP\Provider Gateway for Claude.lnk", p 0)'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend
