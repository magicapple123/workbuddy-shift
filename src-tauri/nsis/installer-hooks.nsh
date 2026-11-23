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
