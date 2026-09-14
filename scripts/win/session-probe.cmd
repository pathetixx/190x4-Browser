@echo off
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0session-probe.ps1" > spike-out\session.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\session.log
