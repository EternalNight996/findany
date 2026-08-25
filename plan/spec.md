# 内容扫描器 ContentSonar — 方案规范

> 用途：在指定目录树下，对文件内容做「包含 / 不包含 关键字」判定，并发扫描，
> 结果输出为 Excel 明细 + 按「out/日期时间/目录名/文件」落盘命中文件。
> 本文件是定版依据（开发架构 / 生产流程 / 拓扑图 / 设计 token / 组件清单 / 契约）。

---

## 1. 交付形态

- **形态**：Windows 桌面单机程序（原生 GUI，双击即用，无需浏览器）。
- **框架**：Python 3.11+ / **PySide6 (Qt)**。核心扫描引擎**不依赖 Qt**，可独立复用、命令行测试。
- **UI 交付**：1 套 HTML+CSS 样板（`plan/ui-mockup.html`，可评审）+ 本 markdown 规范。
- **输出**：Excel 明细（openpyxl，缺失时降级 CSV）+ 命中文件落盘。

## 2. 技术栈与关键取舍

| 层 | 选型 | 取舍理由 |
|----|------|----------|
| GUI | PySide6 (Qt Widgets) | 原生桌面：配置面板/表格/进度/弹窗/目录对话框一步到位，事件循环天然适合异步扫描 |
| 扫描 | `concurrent.futures.ThreadPoolExecutor` | 1~64 线程可配，I/O 密集，线程池足够（无需进程池） |
| 编码 | `chardet` 探测 + 多编码降级 (`utf-8-sig`→`utf-8`→`gbk`→`latin-1`) | 兼容 GBK/UTF-16 等，避免乱码误判 |
| 二进制/超大 | 前置 8KB 头 + 零字节检测 + `max_file_mb` 阈值 | 不读二进制，控制性能与误匹配 |
| Excel | `openpyxl` | 多能力；不可用时降级 `csv`（utf-8-sig 带 BOM，Excel 直接打开） |
| 配置 | `config.json` 持久化 + dataclass | 修改后重启生效，GUI 内可再改 |
| 日志 | Python `logging` + GUI 环形缓冲 | 实时推送弹窗与底部状态条 |

## 3. 开发架构

```
sonar/                        # 后端（无 Qt，可测试）
 ├─ config.py                 # SearchConfig dataclass + config.json 读写 + 默认值/校验
 ├─ scanner.py                # 遍历(walk_files) / 编码探测 / 单文件匹配(match_one)
 │                            # / 引擎(ScanEngine: 线程池+进度回调+取消)
 ├─ exporter.py               # make_batch_dir(日期) / export_excel / copy_hits
 └─ __init__.py

app.py (或 main.py)           # PySide6 GUI 层（仅本层依赖 Qt）
 ├─ MainWindow                # 顶栏 + 左配置 + 右结果 + 底部日志条
 ├─ ConfigPanel               # 目录/关键字/包含-不含/线程/扩展名/编码/选项/输出
 ├─ ResultTable + StatBar     # 表格 + 统计(已扫/命中/未命中/耗时) + 进度
 ├─ LogDialog                 # 日志弹窗(清空/关闭/自动滚底)
 └─ Worker(QThread 包装)      # 后台跑 ScanEngine，Qt 信号→UI 更新

out/                          # 输出根（默认程序目录下）
 └─ <YYYY-MM-DD_HH-MM>/{目录名}/{文件名}   # 命中文件落盘
 └─ <YYYY-MM-DD_HH-MM>/scan_result.xlsx   # Excel 明细
```

**数据流**：ConfigPanel(取配置) → `SearchConfig.validate()` → Worker 线程 `ScanEngine.scan()`
→ (`on_progress`/`on_log` 回调 → Qt 信号 → 更新进度条/日志/表格) → 结束 → `export_excel()` + `copy_hits()`
→ 弹出完成提示 + 打开输出目录。

## 4. 生产流程

1. **配置**：GUI 填目录/关键字/模式/线程/扩展名/编码/选项 → 点「开始扫描」。
2. **校验**：`SearchConfig.validate()`，非法项红字提示，不启动。
3. **收集**：递归/非递归收集符合扩展名的文件（跳过隐藏/`.` 开头的目录与文件）。
4. **并发**：线程池按 `threads` 并行 `match_one`；零字节/超大直接 `skipped`。
5. **判定**：`inc` 模式命中=文件含关键字；`exc` 模式命中=文件**不含**关键字。记录命中行号/次数。
6. **进度**：`done/total/hit/miss/skipped` 实时回调 → UI 更新；可「停止」。
7. **产出**：`export_excel()` 落 `scan_result.xlsx`；`copy_hits()` 把命中文件按 `{目录名}/{文件名}` 落盘到 `out/<日期时间>/`。
8. **收尾**：完成日志 + 提示是否打开输出目录。`config.json` 保存本次配置。

## 5. 拓扑图

```
                            ┌───────────────────────────┐
                            │   MainWindow (QMainWindow) │
                            │  顶栏 · 配置面板 · 结果表格 │
                            │  统计条 · 进度条 · 开始/停止│
                            └─────────────┬─────────────┘
                                          │ Qt Signal/Slot
                 ┌────────────────────────┼───────────────────────────┐
                 ▼                        ▼                           ▼
        ┌─────────────────┐    ┌──────────────────────┐    ┌────────────────────┐
        │  ConfigPanel    │    │  Worker(QThread)     │    │  LogDialog          │
        │  → SearchConfig │───▶│  ScanEngine.scan()   │───▶│  日志弹窗(清空/关)  │
        └─────────────────┘    └───────┬──────────┘    └────────────────────┘
                                       │ 回调 on_progress / on_log
                                       ▼
        ┌───────────────────────────────────────────────┐
        │  sonar.backend（无 Qt，可复用/可 CLI 测试）      │
        │  walk_files → match_one(编码探测/包含-不含)     │
        │  ThreadPoolExecutor(1~64)                      │
        └───────────────────────┬───────────────────────┘
                                ▼
        ┌───────────────────────────────────────────────┐
        │  exporter: export_excel(.xlsx/.csv) + copy_hits│
        │                                          ▼     │
        │   out/<YYYY-MM-DD_HH-MM>/scan_result.xlsx      │
        │   out/<YYYY-MM-DD_HH-MM>/{目录名}/{文件}        │
        └───────────────────────────────────────────────┘
```

## 6. 设计系统 / Token（UI 规范，参考 dsh-theme）

> 运行时由 DSH 注入 `--dsw-alias-*`；HTML 样板内置同值 fallback 用于独立预览。PySide6 侧用同语义的 QSS 颜色/间距。

| 语义 | 映射变量 | 深色值（fallback） | 浅色值 |
|------|----------|--------------------|--------|
| 背景基座 | `--dsw-alias-bg-base` | `#0f0f0f` | `#ffffff` |
| 面板底 | `--dsw-alias-bg-layer-1` | `#151517` | `#fafafa` |
| 卡片/亮层 | `--dsw-alias-bg-layer-2` | `#1c1c1f` | `#f2f2f3` |
| 主品牌色 | `--dsw-alias-brand-primary` | `#5686fe` | `#4176e6` |
| 主文字 | `--dsw-alias-label-primary` | `#f5f5f5` | `#0f0f0f` |
| 次要文字 | `--dsw-alias-label-secondary` | `#979da6` | `#65676b` |
| 成功 | `--dsw-alias-state-success-primary` | `#22c55e` | `#16a34a` |
| 错误 | `--dsw-alias-state-error-primary` | `#ef4444` | `#dc2626` |
| 警告 | `--dsw-alias-state-warn-primary` | `#f59e0b` | `#d97706` |

- **字体**：UI `-apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif`；等宽 `ui-monospace, SFMono-Regular, Consolas, monospace`（用于路径/行号/日志）。
- **字号**：正文 13px，表格 12px，标签 11.5px，统计数字 17px（等宽）。
- **圆角**：_radius-s 6px / _radius-m 9px / _radius-l 12px。
- **间距**：_pad-xs 6px / _pad-s 10px / _pad-m 16px / _pad-l 22px。
- **面板**：左配置 296px 定宽，右侧结果自适应；顶栏 46px，底部日志条 26px。
- **动效**：按钮 hover 背景过渡 0.15s、按下位移 1px、进度条宽度 0.2s、忙碌指示灯脉冲 1s。
- **无渐变/无阴影堆叠**：扁平 + 细分隔线（1px `border-l1/l2`），避免 AI 模板感。

## 7. 组件清单

| 组件 | 说明 |
|------|------|
| 顶栏   | 品牌标 + 「主题切换 / 日志 / 打开输出目录」 |
| 配置面板 | 目录(带「…」对话框)、关键字、包含/不含分段开关、线程滑块+数字(1~64)、扩展名、编码下拉、选项开关（大小写/递归/复制命中/记录未命中）、输出目录 |
| 统计条 | 已扫描 / 命中(绿) / 未命中(红) / 耗时，右侧进度条+百分比 |
| 结果表格 | 序号/目录/文件/相对路径/关键字/命中状态(标签)/命中行/命中次数/大小/修改时间/编码 |
| 日志条   | 底部固定，最新一条 + 忙碌指示灯，点击弹窗 |
| 日志弹窗 | 环形缓冲(最近800条)，按 level 着色(ok绿/warn黄/err红)，支持清空/自动滚底/关闭 |
| 按钮状态机 | 开始(主色) / 停止(红描边，仅运行时可点) / 进行中禁用开始 |

## 8. 产物目录（交付物）

```
findany/
 ├─ sonar/            # 后端（已就绪，微调）
 ├─ app.py            # PySide6 GUI 入口（新建，本次主交付）
 ├─ config.json       # 运行时生成
 ├─ out/              # 输出根
 ├─ plan/
 │   ├─ ui-mockup.html   # UI 评审样板（已就绪）
 │   └─ spec.md          # 本规范
 └─ README.md          # 运行/使用说明
```

## 9. 面壁契约

### 目标
交付 Windows 桌面程序 ContentSonar（PySide6），在选定目录下按扩展名过滤、并发(1~64 线程)扫描文件内容，判定「包含 / 不包含 IT6563」，输出 Excel 明细 + 按「out/日期时间/目录名/文件」落盘命中文件；GUI 含配置面板 / 结果表格 / 统计与进度 / 日志弹窗。

### 分步（可勾选）
- [ ] ① 扫描引擎校验与微调（含/不含判定、编码降级、二进制/超大跳过、取消）
- [ ] ② 输出层对齐「out/日期时间/目录名/文件」命名 + Excel 明细 + 摘要 sheet + CSV 降级
- [ ] ③ PySide6 主窗口：配置面板 + 结果表格 + 统计/进度 + 开始/停止
- [ ] ④ 日志弹窗（实时 / 清空 / 着色 / 自动滚底）+ 底部状态条
- [ ] ⑤ config.json 持久化 + 参数校验与错误提示
- [ ] ⑥ README + 运行说明（含 pyinstaller 打包命令）

### 验收标准（可核对）
1. 选「包含 IT6563」：Excel 中命中行状态=命中且带命中行号；选「不包含 IT6563」结果反选。
2. 输出目录形如 `out/<YYYY-MM-DD_HH-MM>/{目录名}/{文件}`，命中文件落盘、目录名/文件名正确，同名自动加序号。
3. 日志弹窗实时刷新、可按级别着色、清空与关闭可用；底部状态条显示最新一条。
4. 线程数 1~64 可配，并发扫描随线程变化；运行中「停止」可即时中止。
5. 修改配置重启后 `config.json` 生效（持久化）。
6. `openpyxl` 不可用时自动降级为 `*.csv`（utf-8-sig）。
7. 无权限/超大/二进制文件被跳过且记录日志，不中断整批。

### 风险与对策
- **大目录慢**：扩展名过滤 + 跳过二进制/超大 + 线程池。
- **编码乱码/误判**：chardet 探测 + 多编码降级 + 宽容解码。
- **命中文件同名冲突**：落盘自动加序号。
- **只读/无权限文件**：捕获异常记日志，跳过继续。
- **Excel 依赖缺失**：自动降级 CSV 兜底。
```
