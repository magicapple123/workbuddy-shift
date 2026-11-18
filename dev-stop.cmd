@echo off
rem One-click stopper for the dev preview (Windows).
rem Kills only processes that belong to this checkout: the dev app under
rem <repo>\target\... and node/cargo/rustc whose command line mentions this repo.
rem It never touches the installed build or other terminals.
setlocal
title WorkBuddy Shift - Stop Dev Preview
set WB_DEV_STOP_PAUSE=1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\dev-stop.ps1"
