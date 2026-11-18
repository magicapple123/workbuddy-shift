@echo off
rem One-click launcher for the dev preview (Windows).
rem Closes the installed build first (Tauri single-instance lock), then runs
rem `pnpm run tauri dev` in this window. Closing this window stops the server.
setlocal
title WorkBuddy Shift - Dev Preview
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\dev-start.ps1"
if errorlevel 1 (
  echo.
  echo Dev preview exited with an error. Press any key to close.
  pause >nul
)
