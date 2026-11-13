; Hooks into Tauri's NSIS installer (tauri.windows.conf.json). The installer is per user, so these run as
; the user, with no administrator prompt. None of them run when the updater replaces the app ($UpdateMode).

!macro NSIS_HOOK_PREUNINSTALL
  ; "Delete the application data" also deletes the API keys SuperEditr saved in Credential Manager, which
  ; live outside its folders. The app deletes them itself, so only the entries it wrote go. It has to run
  ; before its program is removed.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DetailPrint "Deleting saved API keys"
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" delete-saved-keys' $0
    ${If} $0 <> 0
      DetailPrint "Couldn't delete the saved API keys ($0)"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; Open at login: the Run value SuperEditr writes, and the on-or-off record Task Manager keeps for it.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "supereditr"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "supereditr"
  ${EndIf}
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; The diagnostic log sits in the install folder's Logs folder, which keeps the folder from being removed.
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\SuperEditr\Logs"
    RMDir "$LOCALAPPDATA\SuperEditr"
  ${EndIf}
!macroend
