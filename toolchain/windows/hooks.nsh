; UniDTK — хуки установщика NSIS.
;
; Задача хуков ровно одна: выполнить две идемпотентные команды, которые
; приложение и так умеет делать само. Логика живёт в CLI (`udtk`), а не здесь:
; шаблоны задач, подстановка пути установки и повышение прав — это его работа,
; дублировать её в NSIS значило бы завести второй источник истины.
;
; Установщик собран в режиме `perMachine` (см. tauri.conf.json) и уже работает
; с правами администратора, поэтому регистрация задач с `RunLevel
; HighestAvailable` проходит без второго запроса UAC.
;
; Запуск CLI — через `nsExec::ExecToStack`, а не `ExecWait`:
;   * nsExec не создаёт видимое консольное окно (проверено: тихий прогон
;     установщика не мигает чёрным), а ExecWait порождает окно cmd на весь
;     срок жизни CLI;
;   * nsExec возвращает настоящий код возврата дочернего процесса, а ExecWait
;     его выбрасывает — установщик молча завершался с нулём даже когда
;     `path ensure` или `tasks register` упали (проверено пробами:
;     код 7 -> exit 0, код 3 -> SetErrorLevel 50 -> exit 50).

!macro NSIS_HOOK_PREINSTALL
  ; При установке файлы накладываются поверх старых, но удалённые или
  ; переименованные модули остаются в $INSTDIR — приложение находит их через
  ; readDir ресурсов и рисует вторую «ghost»-карточку (та же болезнь, что
  ; чистит prune_stale_resources в dev-каталоге). Чистим до копирования:
  ; Install section вызывает этот хук до CheckIfAppIsRunning и до File.
  RMDir /r "$INSTDIR\modules"
  RMDir /r "$INSTDIR\fonts"
  RMDir /r "$INSTDIR\scheduler-tasks"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Путь установки — в пользовательский PATH. Копий бинарников не делаем:
  ; `udtk.exe` лежит здесь же, и это единственная его копия.
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" path ensure'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_path_ok 0
    DetailPrint "udtk path ensure failed (code $0): $1"
    SetErrorLevel 11
    Goto uni_hooks_done
  uni_path_ok:
  ; Автозапуск приложения + два повышенных хелпера VPN (start/stop).
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
  ; Сначала снимаем задачи: иначе после удаления они остались бы указывать на
  ; несуществующий `vpn-helper.exe` и молча падали при входе в систему.
  ; Ошибка здесь не должна блокировать удаление файлов — код лишь логируется.
  nsExec::ExecToStack '"$INSTDIR\udtk.exe" tasks unregister all'
  Pop $0
  Pop $1
  StrCmp $0 "0" uni_unreg_ok 0
    DetailPrint "udtk tasks unregister failed (code $0): $1"
  uni_unreg_ok:
!macroend
