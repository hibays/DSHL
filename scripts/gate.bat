@echo off
rem dshl gate — cmd.exe shim: forwards to the PowerShell implementation.
rem Usage: scripts\gate.bat [-Rust|-Js]
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0gate.ps1" %*
exit /b %ERRORLEVEL%
