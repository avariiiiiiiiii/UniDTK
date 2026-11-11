; UniDTK — NSIS installer hooks.
;
; The hooks have exactly one job: run two idempotent commands that
; the application can already do by itself. The logic lives in the CLI (`udtk`), not here:
; task templates, substitution of the install path, and privilege elevation are its job;
; duplicating them in NSIS would create a second source of truth.
;
; The installer is built in `perMachine` mode (see tauri.conf.json) and already runs
; with administrator rights, so registering tasks with `RunLevel
; HighestAvailable` goes through without a second UAC prompt.
;
; The CLI is launched via `nsExec::ExecToStack`, not `ExecWait`:
;   * nsExec does not create a visible console window (verified: a silent installer
;     run does not flash black), whereas ExecWait opens a cmd window for the whole
;     lifetime of the CLI;
;   * nsExec returns the real exit code of the child process, whereas ExecWait
;     discards it: the installer silently exited with zero even when
;     `path ensure` or `tasks register` failed (verified by probes:
;     code 7 -> exit 0, code 3 -> SetErrorLevel 50 -> exit 50).

!macro NSIS_HOOK_PREINSTALL
  ; On install, files are copied over the old ones, but removed or renamed modules remain in
  ; $INSTDIR: the application finds them via readDir of resources and draws a second "ghost" card
  ; (the same disease that prune_stale_resources cleans up in the dev directory). We clean up before copying:
  ; the Install section calls this hook before CheckIfAppIsRunning and before File.
  ;
  ; The hook runs before that check, so it stops a running application itself: the folders
  ; below are in use by it, and deleting them from under a live app (or on an install the
  ; user then cancels) would leave it without its modules. `udtk stop` reports success only
  ; when nothing of the install is left running.
  IfFileExists "$INSTDIR\udtk.exe" 0 uni_pre_clean
    nsExec::ExecToStack '"$INSTDIR\udtk.exe" stop'
    Pop $0
    Pop $1
    StrCmp $0 "0" uni_pre_clean 0
      DetailPrint "udtk stop failed (code $0): $1"
      MessageBox MB_OK|MB_ICONSTOP "UniDTK is still running and could not be stopped. Close it and run the installer again." /SD IDOK
      Abort
  uni_pre_clean:
  RMDir /r "$INSTDIR\modules"
  RMDir /r "$INSTDIR\fonts"
  RMDir /r "$INSTDIR\scheduler-tasks"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; The install path goes into the user PATH. We do not make copies of the binaries:
  ; `udtk.exe` lies right here, and this is its only copy.
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" path ensure'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_path_ok 0
    DetailPrint "udtk path ensure failed (code $0): $1"
    SetErrorLevel 11
    Goto uni_hooks_done
  uni_path_ok:
  ; Application autostart + two elevated VPN helpers (start/stop).
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" tasks register all'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_tasks_ok 0
    DetailPrint "udtk tasks register failed (code $0): $1"
    SetErrorLevel 12
    Goto uni_hooks_done
  uni_tasks_ok:
  uni_hooks_done:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove the tasks first: otherwise after uninstall they would remain pointing at a
  ; nonexistent `vpn-helper.exe` and would silently fail at logon.
  ; An error here must not block file removal; the code is only logged.
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" tasks unregister all'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_unreg_ok 0
    DetailPrint "udtk tasks unregister failed (code $0): $1"
  uni_unreg_ok:
  ; The install directory leaves the User PATH, where the install put it. Also only logged.
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" path remove'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_path_removed 0
    DetailPrint "udtk path remove failed (code $0): $1"
  uni_path_removed:
!macroend
