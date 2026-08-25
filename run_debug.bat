@echo off
chcp 65001 >nul
cd /d "%~dp0"
set "PYEXE="
for /f "delims=" %%i in ('py -3 -c "import PySide6,sys;print(sys.executable)" 2^>nul') do set "PYEXE=%%i"
if not defined PYEXE (
  for /f "delims=" %%i in ('python -c "import PySide6,sys;print(sys.executable)" 2^>nul') do set "PYEXE=%%i"
)
if not defined PYEXE (
  echo 未找到带 PySide6 的 Python。请安装后重试，或运行 build_exe.bat 打包。
  pause
  exit /b 1
)
echo 调试模式：用 %PYEXE% 启动（控制台，出错会暂停）...
%PYEXE% app.py
if errorlevel 1 (
  echo.
  echo [!] 出错：信息如上，或见 crash.log。
  pause
)
