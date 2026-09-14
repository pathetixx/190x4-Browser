; Хуки установщика NSIS (bundle > windows > nsis > installerHooks).
; Что именно пишется в реестр — src/default_browser.rs.

; Браузер появляется в «Приложениях по умолчанию» Windows сразу после установки.
!macro NSIS_HOOK_POSTINSTALL
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --register-browser'
!macroend

; Удаление снимает регистрацию, пока exe ещё на месте. Обновление её не трогает.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --unregister-browser'
  ${EndIf}
!macroend
