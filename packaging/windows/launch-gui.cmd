@echo off
setlocal
"%~dp0ekmp.exe" gui %*
exit /b %errorlevel%
