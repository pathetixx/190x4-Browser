@echo off
rem Zapusk zamera pamyati v interaktivnoy sessii (schtasks /IT).
rem ssh-sessiya zhivet v session 0 i ne vidit rabochiy stol: okno WebView2
rem tam ne sozdaetsya, a bez okna zamer bessmyslen.
cd /d "%~dp0..\.."
if not exist spike-out mkdir spike-out
set RUST_BACKTRACE=1
target\release\190x4-spike.exe memory --tabs 20 --settle 20 > spike-out\memory.log 2>&1
echo EXIT=%ERRORLEVEL% >> spike-out\memory.log
