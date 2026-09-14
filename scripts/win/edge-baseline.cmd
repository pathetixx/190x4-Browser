@echo off
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0edge-baseline.ps1" > spike-out\edge.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\edge.log
