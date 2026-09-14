@echo off
rem Okno ostaetsya otkrytym 3 minuty: nuzhno glazami proverit, igraet li video.
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
set RUST_BACKTRACE=1
target\release\190x4-spike.exe drm --url https://www.kinopoisk.ru/ > spike-out\drm.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\drm.log
