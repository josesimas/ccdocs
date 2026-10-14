@echo off
rem Rebuild (debug) and run ccdocs. Arguments are passed through, e.g.: run-debug.bat --group 100kts --out out
cd /d "%~dp0"
cargo build || exit /b 1
target\debug\ccdocs.exe %*
