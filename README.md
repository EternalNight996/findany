<div align="center">
  <img src="https://img.icons8.com/color/96/magnifying-glass--v1.png" alt="ContentSonar" width="72">
  <h1>ContentSonar</h1>
  <p>目录内容扫描器 —— 在指定目录树下并发检索文件内容，判定「包含 / 不包含」关键字，一键导出 Excel 并落盘命中文件。</p>

  [![Platform](https://img.shields.io/badge/platform-Windows-0078D4?logo=windows)](https://github.com/)
  [![Python](https://img.shields.io/badge/Python-3.10+-3776AB?logo=python)](https://www.python.org/)
  [![UI](https://img.shields.io/badge/UI-PySide6%20(Qt)-41CD52)](https://wiki.qt.io/Qt_for_Python)
  [![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
</div>

> 一次配置，并发扫描，结果即出。生产 / 物料 / 工程等场景里的「有没有出现某个关键字」排查利器。

ContentSonar 是一个 Windows 桌面程序，用 **Python + PySide6** 编写，核心扫描引擎不依赖 Qt、可独立复用。对目录下所有文件做「**包含 / 不包含**」关键字判定，支持并发、编码探测、二进制/超大文件跳过、Excel 明细与摘要输出，并把命中文件按 `out/日期时间/目录/文件` 落盘。

## 功能特性

- `🐍` **零搭环境**：PySide6 原生桌面，双击即用，无需浏览器、无需 Node。
- `📁` **全目录递归**：扫描根目录下所有子目录与文件，按扩展名过滤。
- `🔍` **包含 / 不包含**：两种匹配模式，一次看清「谁命中了 IT6563 / 谁没有」。
- `⚡` **并发扫描**：线程 1~64 可配，I/O 密集，实时进度与统计（已扫 / 命中 / 未命中 / 跳过 / 耗时）。
- `🧾` **Excel 输出**：明细 + 摘要双 sheet；openpyxl 缺失时自动降级为 CSV（utf-8-sig）。
- `📦` **命中文件落盘**：按 `out/<YYYY-MM-DD_HH-MM>/{目录}/{文件}` 复制，同名自动加序号。
- `🧩` **编码智能**：chardet 探测 + 多编码降级（utf-8/GBK/UTF-16），杜绝乱码误判。
- `🛡️` **稳健**：二进制 / 超大文件跳过，输出目录自扫描排除，无权限文件记日志不中断。
- `💬` **日志弹窗**：运行日志实时滚动，按级别着色，可清空 / 关闭。
- `🎨` **深 / 浅主题**：一键切换。

## 系统要求

| 依赖 | 版本 | 说明 |
| --- | --- | --- |
| Windows | 10 / 11 | 桌面运行环境 |
| Python | 3.10+ | 本项目在 3.13 开发 / 验证 |
| PySide6 | 6.6+ | GUI 框架 |
| openpyxl | 3.1+ | Excel 输出（缺失自动降级 CSV） |
| chardet | 5+ | 编码探测 |

## 安装

```bash
pip install -r requirements.txt
```

依赖：`PySide6`、`openpyxl`、`chardet`。

## 运行

双击 **`run.bat`**（无控制台，干净启动）；或调试用 **`run_debug.bat`**（控制台 + 出错暂停显示）；或命令行：

```bash
python app.py
```

> 若启动失败，会写 `crash.log` 并弹错误窗；`run_debug.bat` 会把真实报错暂停打印。

## 使用说明

1. 选择**扫描目录**（「…」浏览）。
2. 输入**关键字**（如 `IT6563`）。
3. 选择**匹配模式**：包含 = 内容含关键字即命中；不包含 = 内容不含关键字即命中。
4. 调整**并发线程数**（1~64）、**扩展名**（逗号分隔，留空=全部）、**编码**、各开关。
5. 点**开始扫描**，右侧实时显示统计与进度，可随时**停止**。
6. 完成弹窗提示，可一键**打开输出目录**。

## 配置

| 配置项 | 说明 | 默认值 |
| --- | --- | --- |
| `root_dir` | 扫描根目录 | 空 |
| `keyword` | 要检索的关键字 | `IT6563` |
| `mode` | `inc` 包含 / `exc` 不包含 | `inc` |
| `threads` | 并发线程数 1~64 | `8` |
| `extensions` | 扩展名过滤（逗号分隔，空=全部） | txt,log,csv,md,xml,json,java,cpp,py,ini,cfg,html |
| `encoding` | auto / utf-8 / gbk / utf-16 / ascii | `auto` |
| `case_sensitive` | 大小写敏感 | `false` |
| `recursive` | 递归子目录 | `true` |
| `copy_files` | 将命中文件复制到 out | `false` |
| `record_miss` | 在 Excel 记录未命中文件 | `true` |
| `out_dir` | 输出根目录 | 程序目录下 `out/` |
| `max_file_mb` | 超过视为超大/二进制并跳过 | `20` |

配置持久化到 `config.json`（程序目录下，启动自动加载、扫描时保存）。

## 输出

```
<程序目录>/out/
 └─ <YYYY-MM-DD_HH-MM>/           # 本次批次（同分钟自动加序号，不覆盖）
     ├─ scan_result.xlsx          # 明细 + 摘要
     └─ {目录}/{文件}             # 命中文件（勾选「复制到 out」才复制）
```

明细列：序号、相对路径、目录、扩展名、包含状态、命中行号、命中行内容、匹配计数、大小、修改时间、编码、绝对路径。

## 一键打包 EXE

双击 **`build_exe.bat`** 或命令行运行：

```bash
pip install pyinstaller
python -m PyInstaller --noconfirm --clean --windowed --onefile --name ContentSonar app.py
```

产物：`dist/ContentSonar.exe`（可分发，免装 Python）。

> 可加自定义图标：准备 `icon.ico` 后，在 `build_exe.bat` 的 PyInstaller 命令加 `--icon icon.ico`。

## 目录结构

```
ContentSonar/
 ├─ app.py               # PySide6 GUI 入口
 ├─ sonar/               # 后端（无 Qt，可复用）：config / scanner / exporter
 ├─ plan/                # UI 样板 + 方案规范 (ui-mockup.html, spec.md)
 ├─ run.bat              # 无控制台启动
 ├─ run_debug.bat        # 控制台启动（出错暂停显示）
 ├─ build_exe.bat        # 一键打包
 ├─ requirements.txt
 ├─ LICENSE
 ├─ config.json          # 运行时生成
 ├─ crash.log            # 启动失败时生成
 └─ out/                 # 输出根
```

## 发布到 GitHub / Gitee

本项目支持双远端发布（GitHub + Gitee）。先建好两个仓库（如同名 `ContentSonar`），双击 **`publish.bat`** 或：

```bash
git init
git add -A
git commit -m "init: ContentSonar v1.0"

# GitHub
git remote add origin https://github.com/<你的用户名>/ContentSonar.git
# Gitee
git remote add gitee  https://gitee.com/<你的用户名>/ContentSonar.git

git push -u origin master
git push -u gitee  master
```

> `build/`、`dist/`、`out/`、`config.json`、`crash.log`、`__pycache__` 已写入 `.gitignore`，不会提交。

## 许可证

[MIT](LICENSE)
