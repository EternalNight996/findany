@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo === ContentSonar 调试模式（出错会暂停并显示）===
python app.py
if errorlevel 1 (
  echo.
  echo [!] 启动出错，如上。请查看 crash.log。
  pause
)
