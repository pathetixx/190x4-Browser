@echo off
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0find-probe.ps1" > spike-out\find.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\find.log
