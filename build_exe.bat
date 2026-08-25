@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo === ContentSonar 一键打包 ===
python -m PyInstaller --noconfirm --clean --windowed --onefile --name ContentSonar app.py
if errorlevel 1 (
  echo.
  echo [!] 打包失败看上方报错。
  pause
) else (
  echo.
  echo [完成] 产物：dist\ContentSonar.exe
  pause
)
