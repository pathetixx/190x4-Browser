@echo off
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0proc-probe.ps1" > spike-out\proc.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\proc.log
