@echo off
chcp 65001 >nul
echo === findany 环境检查 ===
echo.
echo [1] 当前 python 命令解析到谁:
where python
python -c "import sys;print('    python ->', sys.executable)" 2>nul
echo.
echo [2] 该 python 能否导入 PySide6:
python -c "import PySide6;print('    PySide6 OK', PySide6.__version__)" 2>nul
if errorlevel 1 echo    ^(该 python 没有 PySide6，或 python 是商店存根，根本没执行代码^)
echo.
echo [3] 用官方启动器 py 试:
py -3 -c "import sys,PySide6;print('    py -3 ->', sys.executable, '| PySide6', PySide6.__version__)" 2>nul
echo.
echo 结论: 若 [1][2] 输出了 WindowsApps 或根本无输出/报错，说明 python 被微软商店存根截胡，
echo       并不会真正运行。此时用 py -3 app.py，或直接双击 run.bat（已自动用 py -3）。
echo.
pause
