; 覆盖安装前结束常驻的 companion-runtime.exe（内置会话悬浮窗的采集运行时）。
;
; 为什么需要：它由主程序在启用悬浮窗时拉起并常驻，Tauri 的 NSIS 模板只会结束
; **主程序**，不会碰这个子进程。于是安装器写 `$INSTDIR\companion-runtime.exe`
; 时会拿到共享冲突，弹出
;
;     Error opening file for writing: ...\companion-runtime.exe
;
; 用户只能选「忽略」继续，安装包里那一个文件就停在旧版本（v0.3.1 的 issue #5
; 有用户实际撞到；0.3.1 该文件与本版功能一致才没造成影响，但这是运气，不能依赖）。
;
; 时机：Tauri 在复制文件、写注册表、建快捷方式**之前**执行本宏，此时上游的
; CheckIfAppIsRunning 已经把主程序关掉，杀掉子进程不会被立刻重新拉起。
; taskkill 失败（进程不存在 / 权限不足）不影响安装，所以不判返回值。
!macro NSIS_HOOK_PREINSTALL
  DetailPrint "停止 companion-runtime.exe（避免覆盖安装时文件被占用）"
  nsExec::Exec 'taskkill /F /IM companion-runtime.exe'
  Pop $0
  ; 进程退出到句柄真正释放有一小段延迟，给安装器留出覆盖文件的时间。
  Sleep 500
!macroend

; 卸载前清理注册在 WorkBuddy / CodeBuddy IDE / Codex 配置里的 hook（issue #20）。
; 只摘本工具和旧 .agent-studio 品牌的条目，积分签到的 hook.sh 与第三方 hook 不动；
; 同时写 opt-out（integrations.json），防止残留的 daemon 再次自动装回。
!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "清理 WorkBuddy Shift 注册的 hooks"
  nsExec::Exec 'taskkill /F /IM companion-runtime.exe'
  Pop $0
  nsExec::Exec 'taskkill /F /IM companion-runtime-v1.exe'
  Pop $0
  ; 卸载文件还没删，优先用随包发布的 CLI。
  IfFileExists "$INSTDIR\companion-runtime.exe" 0 skip_uninstall_installed_cli
    nsExec::Exec '"$INSTDIR\companion-runtime.exe" uninstall-hooks --home "$PROFILE"'
    Pop $0
  skip_uninstall_installed_cli:
  ; 兜底：用户目录里的稳定 hook 入口（daemon 每次启动都会发布）。
  IfFileExists "$PROFILE\.workbuddy-shift-companion\bin\companion-runtime-v1.exe" 0 skip_uninstall_stable_cli
    nsExec::Exec '"$PROFILE\.workbuddy-shift-companion\bin\companion-runtime-v1.exe" uninstall-hooks --home "$PROFILE"'
    Pop $0
  skip_uninstall_stable_cli:
!macroend

; Tauri 在文件/注册表删除之后才调用本宏。此时 $INSTDIR 里的 exe 已不存在，
; 但用户目录下的稳定入口还在，再跑一次确保 opt-out 与注册项都清干净。
!macro NSIS_HOOK_POSTUNINSTALL
  IfFileExists "$PROFILE\.workbuddy-shift-companion\bin\companion-runtime-v1.exe" 0 skip_postuninstall_cli
    nsExec::Exec '"$PROFILE\.workbuddy-shift-companion\bin\companion-runtime-v1.exe" uninstall-hooks --home "$PROFILE"'
    Pop $0
  skip_postuninstall_cli:
!macroend
