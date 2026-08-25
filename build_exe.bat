@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo === findany 一键打包（自包含单文件 exe，免装 Python/依赖库）===
echo 清理旧构建状态...
if exist build ( rmdir /s /q build )
if exist findany.spec ( del /q findany.spec )
echo 检查 PyInstaller...
python -m PyInstaller --version >nul 2>nul
if errorlevel 1 (
  echo 未检测到 PyInstaller，正在安装...
  python -m pip install pyinstaller
)
echo 开始打包...
python -m PyInstaller --noconfirm --clean --windowed --onefile --name findany app.py
if errorlevel 1 (
  echo.
  echo [!] 打包失败看上方报错。
  pause
) else (
  echo.
  echo [完成] 产物：dist\findany.exe  （自包含单文件，双击即用，无需安装 Python/依赖库）
  pause
)
