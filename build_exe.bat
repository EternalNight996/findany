@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo === findany 一键打包（自包含单文件 exe，免装 Python/依赖库）===
echo 清理旧构建状态并释放残留进程...
taskkill /f /im findany.exe >nul 2>nul
if exist build rd /s /q build 2>nul
if exist build del /q build 2>nul
if exist findany.spec del /q findany.spec 2>nul
if exist dist\findany.exe del /q dist\findany.exe 2>nul
echo 检查 PyInstaller...
python -m PyInstaller --version >nul 2>nul
if errorlevel 1 (
  echo 未检测到 PyInstaller，正在安装...
  python -m pip install pyinstaller
)
echo 开始打包...
python -m PyInstaller --noconfirm --windowed --onefile --name findany app.py
if errorlevel 1 (
  echo.
  echo [!] 打包失败看上方报错。若仍报 base_library 目录问题，请手动删除整个 build 文件夹后重试。
  pause
) else (
  echo.
  echo [完成] 产物：dist\findany.exe （自包含单文件，双击即用，无需安装 Python/依赖库）
  pause
)
