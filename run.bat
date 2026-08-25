@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul
cd /d "%~dp0"
set "PYEXE="
for /f "delims=" %%i in ('py -3 -c "import PySide6,sys;print(sys.executable)" 2^>nul') do set "PYEXE=%%i"
if not defined PYEXE (
  for /f "delims=" %%i in ('python -c "import PySide6,sys;print(sys.executable)" 2^>nul') do set "PYEXE=%%i"
)
if defined PYEXE (
  for %%p in ("!PYEXE!") do set "PYDIR=%%~dpp"
  if exist "!PYDIR!pythonw.exe" (
    echo 以无控制台方式启动 findany（源码）...
    start "" "!PYDIR!pythonw.exe" app.py
  ) else (
    echo 启动 findany（源码）...
    start "" "!PYEXE!" app.py
  )
  exit /b 0
)
if exist dist\findany.exe (
  echo 未找到带 PySide6 的 Python，改用打包版 exe（自带解释器）...
  start "" dist\findany.exe
  exit /b 0
)
echo 未找到可用的 Python，且无 dist\findany.exe。
echo 请安装 Python 并执行 pip install -r requirements.txt，或运行 build_exe.bat 打包。
pause
