@echo off
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
set RUST_BACKTRACE=1
target\release\190x4-spike.exe adblock --url https://www.kinopoisk.ru/ --lists src-tauri\lists > spike-out\adblock.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\adblock.log
