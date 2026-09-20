<div align="center">
  <img src="https://img.icons8.com/color/96/magnifying-glass--v1.png" alt="findany" width="72">
  <h1>findany</h1>
  <p>目录内容扫描器 —— 在指定目录树下并发检索文件内容，判定「包含 / 不包含」关键字，一键导出 Excel 并落盘命中文件。</p>

  [![Platform](https://img.shields.io/badge/platform-Windows-0078D4?logo=windows)](https://github.com/)
  [![Python](https://img.shields.io/badge/Python-3.10+-3776AB?logo=python)](https://www.python.org/)
  [![UI](https://img.shields.io/badge/UI-PySide6%20(Qt)-41CD52)](https://wiki.qt.io/Qt_for_Python)
  [![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
</div>

<div align="center">
  <img src="assets/screen/screenshot.png" alt="findany 界面" width="100%" style="border-radius:10px; border:1px solid rgba(128,128,128,.25)">
</div>

> 一次配置，并发扫描，结果即出。生产 / 物料 / 工程等场景里的「有没有出现某个关键字」排查利器。

findany 是一个 Windows 桌面程序，用 **Python + PySide6** 编写，核心扫描引擎不依赖 Qt、可独立复用。对目录下所有文件做「**包含 / 不包含**」关键字判定，支持并发、编码探测、二进制/超大文件跳过、Excel 明细与摘要输出，并把命中文件按 `out/日期时间/目录/文件` 落盘。

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
- `🧭` **日志筛选 / 数据回传**：内置 etest(OA3) / etest / e-autotest 三类判型提取，通用 CLI 回传（intunehelper 预设）+ dry-run + 回传进度条 + 完成倒计时自动关。
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
# 或使用 Python 官方启动器（推荐，可绕开微软商店的 python 存根）
py -3 app.py
```

> **若 `python app.py` 点了没反应**：多半是系统的 `python` 被**微软商店存根**（`C:\...\WindowsApps\python.exe`）截胡了，它不会真正运行 Python。请改用 `py -3 app.py`，或直接双击 `run.bat`（它会优先用 `py -3`，找不到再自动回退到打包版 `dist\findany.exe` 跑 GUI）。若想彻底让 `python app.py` 可用，二选一：
>
> 1. **关闭应用执行别名**：设置 → 应用 → 高级应用设置 → 应用执行别名 → 把 `python.exe` 和 `python3.exe` 的开关**关掉**（之后 `python` 会落到已安装的 Python 上）。
> 2. **调整 PATH**：把 `C:\Users\<你>\AppData\Local\Programs\Python\Python313\`（及其 `Scripts\`）移到 `%LOCALAPPDATA%\Microsoft\WindowsApps\` 之前。
>
> 也可双击 `check_env.bat` 一键诊断，看 `python` 到底解析到谁、有没有 PySide6。
>
> 若启动失败，会写 `crash.log` 并弹错误窗；`run_debug.bat` 会把真实报错暂停打印。

## 使用说明

1. 选择**扫描目录**（「…」浏览）。
2. 输入**关键字**（如 `IT6563`）。
3. 选择**匹配模式**：包含 = 内容含关键字即命中；不包含 = 内容不含关键字即命中。
4. 调整**并发线程数**（1~64）、**扩展名**（逗号分隔，留空=全部）、**编码**、各开关。
5. 点**开始扫描**，右侧实时显示统计与进度，可随时**停止**。
6. 完成弹窗提示，可一键**打开输出目录**。

## 日志筛选 / 数据回传（工作模式切换）

顶栏「工作模式」切到 **日志筛选** 后，左侧出现筛选 / 回传面板：

| 配置项 | 说明 | 默认 |
| --- | --- | --- |
| 筛选类型 | 自动识别 / etest(OA3) / etest / e-autotest / 海格旧测试2 / 海格旧测试3 | 自动识别 |
| 界面（筛选页） | 与通用扫描**共享**：扫描目录 / 输出目录 / 并发线程数 / 编码 / 文件扩展名 / 选项（递归子目录、将命中文件复制到 out=留存命中日志）；扫描专属项（关键字 / 匹配模式 / 大小写敏感 / 记录未命中）自动**隐藏** | — |
| 判型规则 | 文件名 `AUTO2`→e-autotest；`IFT-START`/`SN` 前缀→海格旧测试2；`IFT/CLEAN/BURN/FFT/BATTERY/BFT*` 前缀→海格旧测试3；含 `OA3 inject Start`/`<HardwareHash>`→etest(OA3)；首行含 `: e-autotest`→e-autotest；尾部 `R<{`→etest；其余=未知（对齐 heg-admin-log `parse_path_type` 顺序） | — |
| 启用数据回传 | 提取成功后逐台调第三方 CLI；自动模式下仅 etest(OA3) 参与回传 | 关 |
| dry-run | 只组包校验四字段，不调 CLI、不碰网 | 开 |
| CLI 路径 | 空=程序目录（或 `doc/devicehashupload/`）下 `intunehelper_cli.exe` | 空 |
| SecretKey | 存 `config.json`（已 gitignore，不进源码/日志，按交付 SOP 第 6 条） | 空 |
| 参数模板 | 占位符 `~key~`：`~secret_key~` / `~payload~` / 提取字段；含 `~payload~` 走参数否则写 stdin | `upload --stdin --secret-key ~secret_key~` |
| 超时 / 重试 | 单台 CLI 超时；退出码 21 自动重试（1/2/4s 退避），10/12/20/30 不重试 | 60s / 3 次 |
| 倒计时自动关 | 完成后倒计时归零自动退出程序；弹窗可取消 / 延时 30s / 打开输出目录；**数据为空或回传异常时不关** | 3s，默认关 |
| 自开始扫描 | 工具栏勾选「自开始扫描」（随配置保存）：下次启动程序自动开始扫描/筛选，无需点击 | 默认关 |
| 回传方案 | 方案一 `sn_dir`：SN 关联日志（文件名或内容命中，**多文件**回传）；方案二 `single`：单文件筛选回传。由 TOML 配置或 CLI 参数指定 | — |

**回传判定**（移植 etest-core `check_result` 思路，规则可配）：退出码 0 且 stdout `status∈{accepted, duplicate_accepted}` 双确认=成功；退出码 12=冲突转人工（黄）；其余=失败（红）。结果审计落 `out/<时间>/upload-result.csv`（不含 Hash 与 SecretKey）。

**产物**：`out/<YYYY-MM-DD_HH-MM>/` 下 `filter_result.xlsx`、`upload-result.csv`（回传审计）、命中日志留存。
Excel 明细为**统一模板 36 列**（6 组逻辑排序：识别→设备→网络→OA3→原始→结果，OA3 组含 **HardwareHash 本体** 4000 字符全值）+ 摘要 sheet；**批次自适应**：本批整列全空自动隐藏、列宽按内容自适应、冻结表头+序号/文件列（openpyxl 缺失降级 CSV）。
另有**按判型动态生成的专属 sheet**（etest(OA3)/etest/e-autotest/海格旧测试2/海格旧测试3 各用各的列集，批内没有的类型不生成；未知类型只落总表兜底）。新判型在 `report.py TYPE_TEMPLATES` 登记即获得专属模板。

**自校验**（对 doc/etest-log 6 份生产样例，71 条断言）：

```bash
py -3 tests\\test_logfilter.py    # 输出 ALL PASS，退出码 0
```

## 日志标准

| 运行方式 | 日志文件 | 内容 |
|---|---|---|
| CLI / TOML 自动化（auto_start=true 或 --sn/--file） | logs/findany.log | 启动模式、会话横幅、自动化全流程、异常堆栈 |
| GUI 人工操作 | logs/findany-gui.log | 同上 + 全部界面操作日志（筛选/回传/保存等） |

5MB 自动轮转（.log.1）；旧的 findany-run/startup/crash 散文件已并入标准日志。

## TOML 自动化（检测 → 回传 → 倒计时关）

程序目录放 **`findany.toml`**（或 `findany.exe --config 路径.toml`），`run.auto_start = true` 即启动后自动开跑，完成按倒计时自动关——产线无人值守。

> **首次运行自动生成**：启动时若找不到 toml，程序会输出一份默认模板（含全字段注释）并提示路径；编辑 `root_dir`/`scheme` 后把 `run.auto_start` 改 `true` 即生效。模板已 gitignore（防后续填入的 SecretKey 入库）。
>
> **GUI「保存配置」双写**：点保存同时更新 config.json 与 findany.toml（按节合并：保留注释与 `run.auto_start` 手工开关，缺失键自动追加）；即 GUI 改完，自动化 toml 立即生效。

```toml
[filter]
root_dir = "D:\\logs"          # 方案一：SN 检索根目录
log_type = "auto"              # auto|etest(OA3)|etest|e-autotest|海格旧测试2|海格旧测试3
recursive = true

[run]
auto_start = true
countdown_sec = 3              # 完成后倒计时，归零自动关
auto_close = true

[upload]
enabled = true
dry_run = false                # 上线前先 true 演练
types = ["etest(OA3)"]
cli_path = ""
secret_key = ""
args = "upload --stdin --secret-key ~secret_key~"
timeout_sec = 60
max_retries = 3

[scheme]
mode = "sn_dir"                # sn_dir=方案一 | single=方案二
sn = ""                        # 方案一：设备 SN（--sn 可覆盖）
file = ""                      # 方案二：单文件路径（--file 可覆盖）
```

命令行：`findany.exe --config findany.toml [--sn 设备SN] [--file 单文件]`（`--sn`/`--file` 覆盖 toml 并隐含对应方案）。

## 配置

| 配置项 | 说明 | 默认值 |
| --- | --- | --- |
| `root_dir` | 扫描根目录**或单个文件**（GUI 行内「目录…」/「文件…」双按钮；指向文件时按单文件处理，两种工作模式通用） | 空 |
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

## 日志

程序日志统一写入 `logs/`（`logs/` 已 gitignore），按运行方式分文件（5MB 自动轮转）：

```
logs/
 ├─ findany.log          # CLI / TOML 自动化运行
 └─ findany-gui.log      # GUI 人工操作
```

## 一键打包 EXE

双击 **`build_exe.bat`** 或命令行运行：

```bash
pip install pyinstaller
python -m PyInstaller --noconfirm --clean --windowed --onefile --name findany app.py
```

产物：`dist/findany.exe`（可分发，免装 Python）。

> 可加自定义图标：准备 `icon.ico` 后，在 `build_exe.bat` 的 PyInstaller 命令加 `--icon icon.ico`。

## 目录结构

```
findany/
 ├─ app.py               # PySide6 GUI 入口
 ├─ sonar/               # 后端（无 Qt，可复用）：config / scanner / exporter
 │   └─ logfilter/       # 日志筛选与回传：types(判型) / extractors(提取) / uploader(CLI回传) / engine(编排) / report(产物)
 ├─ tests/               # 自校验（判型/提取/判定/重试，71 条断言）
 ├─ plan/                # UI 样板 + 方案规范 (ui-mockup.html, spec.md)
 ├─ run.bat              # 无控制台启动
 ├─ run_debug.bat        # 控制台启动（出错暂停显示）
 ├─ build_exe.bat        # 一键打包
 ├─ requirements.txt
 ├─ LICENSE
 ├─ check_env.bat        # 环境自检（python 解析/是否有 PySide6）
 ├─ config.json          # 运行时生成
 ├─ logs/                # 运行日志（findany-*.log，已 gitignore）
 └─ out/                 # 输出根
```

## 发布到 GitHub / Gitee（SSH）

本项目支持双远端发布（GitHub + Gitee），通过 **SSH** 推送。先在两平台建好同名空仓库（`findany`），并配置 SSH 公钥：

```bash
# 1. 生成 SSH 密钥（回车三次即可，已有则跳过）
ssh-keygen -t ed25519 -C "you@example.com"

# 2. 查看并复制公钥
cat %USERPROFILE%\.ssh\id_ed25519.pub

# 3. 到 GitHub / Gitee 设置 → SSH Keys → 添加该公钥
```

然后双击 **`publish.bat`**（先把文件顶部 `GH` / `GITEE` 改成你的 SSH 地址），或手动：

```bash
git init
git branch -M main
git add -A
git commit -m "init: findany v1.0"

# GitHub（SSH）
git remote add origin git@github.com:<你的用户名>/findany.git
# Gitee（SSH）
git remote add gitee  git@gitee.com:<你的用户名>/findany.git

git push -u origin main
git push -u gitee  main
```

> 用途说明：
> - SSH 方式推送，无需每次输入账号密码，需先在本地 `ssh-keygen` 生成密钥并在两端添加公钥。
> - 若提示 `Permission denied (publickey)`，检查公钥是否已添加、`ssh -T git@github.com` 是否能通。
> - `build/`、`dist/`、`out/`、`config.json`、`crash.log`、`__pycache__` 已写入 `.gitignore`，不会提交。

## 许可证

[MIT](LICENSE)
