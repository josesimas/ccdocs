@echo off
rem Rebuild (release) and run ccdocs. Arguments are passed through, e.g.: run-release.bat --group 100kts --out out
cd /d "%~dp0"
cargo build --release || exit /b 1
target\release\ccdocs.exe %*
