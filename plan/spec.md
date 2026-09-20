# 内容扫描器 findany — 方案规范

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
交付 Windows 桌面程序 findany（PySide6），在选定目录下按扩展名过滤、并发(1~64 线程)扫描文件内容，判定「包含 / 不包含 IT6563」，输出 Excel 明细 + 按「out/日期时间/目录名/文件」落盘命中文件；GUI 含配置面板 / 结果表格 / 统计与进度 / 日志弹窗。

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

---

## 10. v1.1 增补：日志筛选 / 数据回传（契约对照）

> 定版问答（全A）落档。承载：findany 内新增「工作模式」，不动通用扫描路径。

| # | 契约 | 实现 | 验证（可执行断言） |
|---|------|------|--------------------|
| 1 | 判型：etest(OA3)=doc样例格式；etest=同平台无OA3；e-autotest=AUTO2前缀/首行`: e-autotest`；自动判型，未知落清单 | `sonar/logfilter/types.py::detect_log_type` 五级优先 | 测试 detect 4 条断言 |
| 2 | 回传范围：OA3 默认提 SN/PKID/Hash/Baseboard 回传；etest/e-autotest 默认只提取；回传开关可调 | `FilterRunCfg.upload_types`（默认 `["etest(OA3)]"`），GUI 自动模式下仅 OA3 | 引擎 dry-run dry=3/6 |
| 3 | 通用回传全可配（CLI/参数模板~key~/stdin/超时/重试/判定） | `uploader.UploadProfile` + config.json + GUI 面板；intunehelper 预设 | 参数渲染断言 |
| 4 | 判定：退出码0 且 status∈{accepted,duplicate_accepted} 双确认；12黄；10/20/30红；21 重试 | `uploader.run_upload`（移植 etest-core check_result 思路） | 桩测 5 条（ok/conflict/retry/fail/字段不全） |
| 5 | 手动开始→筛选→回传→倒计时(30s可配)自动关，可取消/延时 | GUI 工作模式切换 + `CountdownDialog` | 离屏冒烟 |
| 6 | dry-run 全量 + 真传 1 台；SecretKey 进 config.json(gitignore) | dry-run 只组包；真传走 `doc/devicehashupload` CLI | 6/6 断言 + 真传 accepted(rid 7854aba1…) |

**锚点来源**：`doc/etest-log/extract-oa3.ps1`（ASCII 锚点逐条移植 Python）；期望值基准 `oa3-samples.json`。
**陷阱移植**：OA3 正文块每台出现 2 次 → 取第 1 次 + 记 `oa3_block_count`；`R<{...}>R` 取最后一个；`OEMRevision` 不误命中（锚点含 `msg="`）。
**产物**：`out/<时间>/filter_result.xlsx + upload-result.csv + 命中日志留存`；审计不含 Hash/SecretKey（SOP 第 6 条）。
**筛选模式固定扫 `.log`**（不沿用通用扫描扩展名，避免卷入采样 csv/json）。
**自校验**：`py -3 tests\\test_logfilter.py` → 97 条断言 ALL PASS。

### 10.1 v1.2 增补：对齐 heg-admin-log 的 e-autotest / 海格旧测试解析

| 项 | 内容 | 验证 |
|---|------|------|
| 判型扩容 | `IFT-START`/`SN` 前缀→海格旧测试2；`IFT/CLEAN/BURN/FFT/BATTERY/BFT-SL/BFT`→海格旧测试3（文件名前缀优先，顺序同 `parse_path_type`） | BURN.log 真样例 + IFT-START 优先级断言 |
| e-autotest 增强 | 尾部 JSON `app_tag` 分发：UUID校验/系统SN校验/板卡SN校验/BIOS版本校验/系统激活|自动化激活→OS激活码/MAC获取（friendly_name 分类 + if_type 兜底，MAC 去横杠） | 0015 真值：system_sn/bios_version/lan |
| 海格旧测试3 | `@OS激活码=/@UUID=/@BIOS_SN=/@BOARD_SN=/@BIOS版本=`（BURN_IGNORE 过滤）+ `<ProductKey>/</ProductKeyID>` + `@网络MAC=[...]` JSON（虚拟卡剔除，MAC 保留横杠） | 合成夹具 9 断言 |
| 海格旧测试2 | 同 @ 锚点 + 多行「接口」块（MAC 在接口行后第 3 行 `MAC地址: `) | 合成夹具 6 断言 |
| 字段对齐 | production_num/system_sn/board_sn/uuid/bios_version/os_key/oa3_key/oa3_id/lan/wifilan/bluetooth（= `DataTaskSigle`）进明细列 | 引擎/报表列扩充 |
| 修复移植 | 上游 `trim_data_list2` 的 contains 反条件按意图修复为去重 push | 夹具断言 |
| 默认策略 | 海格旧测试 2/3 只提取不回传（类型闸默认仅 etest(OA3)） | 引擎 dry-run dry=3 |

### 10.2 v1.3 增补：TOML 自动化 + 回传双方案

| 项 | 内容 | 验证 |
|---|---|---|
| 倒计时默认 | 30s → **3s**（归零自动关程序） | GUI spin 默认值断言 |
| TOML 配置 | `findany.toml`（`[filter]/[run]/[upload]/[scheme]`）或 `--config`；`auto_start=true` 启动即自动跑完整流程 | toml 解析断言 |
| 方案一 sn_dir | 动态 SN 关联日志：文件名或内容命中（64MB 读入上限），**多文件**逐台回传 | 0015→1 份；前缀→6 份；PKID 内容命中→1 份 |
| 方案二 single | 指定单文件筛选回传 | 引擎 total=1 断言 |
| CLI 参数 | `--sn` / `--file` 覆盖 toml 并隐含对应方案 | resolve_auto |
| 模板自生成 | toml 不存在（默认路径或 --config 路径）→ 输出默认模板 + GUI 提示；不覆盖已有；已 gitignore（防 SecretKey 入库） | 生成/不覆盖/显式路径/用户配置 5 断言 |

### 10.3 v1.4 增补：筛选页干净空间（共享项关联使用）

| 项 | 内容 | 验证 |
|---|---|---|
| 共享保留 | 扫描目录 / 输出目录 / 并发线程数 / **编码** / **文件扩展名** / 选项（递归 + 复制命中到 out）在筛选页可见且真实生效 | GUI 断言可见+可用 |
| 编码接通 | `read_text(encoding)` 编码链（auto/utf-8/gbk/utf-16/ascii）→ 引擎逐文件使用 | GBK 字节双链断言 |
| 扩展名接通 | 引擎走 GUI 扩展名（默认 log）；SN 检索同样按白名单过滤 | ext=log/csv/xlsx 三断言 |
| 留存去重 | 删筛选组内重复的「留存命中日志」勾选，由共享「将命中文件复制到 out」接管 | keep_check 移除断言 |
| 隐藏项 | 关键字 / 匹配模式 / 大小写敏感 / 记录未命中（切换模式显示/隐藏，非禁用） | 隐藏残留=空 断言 |

### 10.4 v1.5 增补：统一 Excel 模板优化

| 项 | 内容 | 验证 |
|---|---|---|
| 列序分组 | 35 键集合不变，按 6 组重排：识别→设备→网络→OA3→原始→结果 | 键集合+顺序断言 |
| 空列自动隐藏 | 本批整列全空 → 列隐藏（模板统一、视图自适应）；真批次 35→30 可见 | 合成+真批次断言 |
| 宽度自适应 | 固定偏好（SHA-256=20 等）+ 其余按内容自适应（上限 40）；修旧宽度表 26/35 错位 | 宽度断言 |
| 冻结窗格 | `C2`：冻结表头行 + 序号/文件列 | freeze_panes 断言 |

### 10.5 v1.6 增补：按判型动态生成专属 sheet

| 项 | 内容 | 验证 |
|---|---|---|
| 类型模板表 | `TYPE_TEMPLATES`：etest(OA3)=识别+OA3全量+原始；etest=识别+原始；e-autotest/海格旧测试2=识别+设备+原始；海格旧测试3=识别+设备+PKID核+原始；尾接结果列 | 列序断言 |
| 动态生成 | 批内出现的类型才建 sheet（标题=判型名）；未知类型只落总表；新类型登记模板即生效 | sheetnames 断言 |
| 混合批次 | 真样例 6 文件三类型 → 总表 6 行 + 3 个专属 sheet，各列集/空列隐藏独立 | 端到端脚本 |
| 共用渲染 | `write_sheet` 统一表头样式/空列隐藏/宽度/冻结，总表与类型表同源 | 复用重构 |

### 10.6 v1.7 增补：日志标准化 + 回传收尾语义 + 配置保存

| 项 | 内容 | 验证 |
|---|---|---|
| 日志标准 | CLI/TOML 自动化→logs/findany.log；GUI→logs/findany-gui.log（启动时按模式路由，5MB 轮转）；旧 run/startup/crash 散文件并入 | 双模式端到端：cli 模式自退出且仅 findany.log；gui 模式仅 findany-gui.log |
| 回传收尾 | 真传全部成功→界面 PASS+倒计时关；有失败/冲突→FAIL+弹窗列异常设备、不关；dry-run/未启用维持原行为 | GUI 五场景冒烟（桩注入 run_upload） |
| 配置保存 | 新增「保存配置」按钮（绿色已保存✓闪烁1.2s/失败红）+ closeEvent 兜底落盘 | 保存/关窗双通道冒烟 |
| toml 同步 | 保存按钮双写 config.json + findany.toml：按节合并（受管键改值/缺失追加），保留注释与 run.auto_start；tomllib 往返一致 | 8 断言 + 按钮联动冒烟 |
| 空数据拦截 | 扫描 0 文件或提取 0 条 → FAIL 红标 + 弹窗拦截：不判成功、不倒计时不关（自动化空跑必被看到） | 空目录/对照批次冒烟 |
| 自开始扫描 | 工具栏「自开始扫描」勾选随 config.json 持久化；无 TOML 自动化时启动即自动开跑 | 勾选落盘断言 |
| HardwareHash 落表 | 明细/OA3 专属 sheet 增列 HardwareHash 本体（4000 字符全值，列宽锁 20）；修引擎导出字段拷贝清单缺项 | 真批次双 sheet 全值断言 |

### 10.7 v1.8 增补：GUI 单文件扫描（定稿：单行双按钮）

| 项 | 内容 | 验证 |
|---|---|---|
| 单行双按钮 | 「扫描目录/文件」一行：输入框 + 「目录…」+「文件…」；指向目录按目录扫，指向文件按单文件处理（isfile 判定，字段仍为 root_dir，无额外字段） | 双模式冒烟 |
| 双模式通用 | 通用扫描：ScanEngine walk 旁路（关键字匹配照常）；日志筛选：FilterWorker 走 file_list 通道（判型/回传照常） | 单文件 1 行 / 目录 6 行 PASS 冒烟 |
| 持久化 | root_dir 原样落盘 config.json；scan_file 临时字段已回收 | 无残留键断言 |
| 面板收放 | 「扫描配置」右上角「收/展」按钮（最小两字宽 56px）折叠配置内容；收起面板缩至标题行宽，展开恢复 440px；目录/文件按钮同锁两字宽 | 收放冒烟 + 142 回归 |
| 自动化交互 | auto_pending 时完成不弹询问框；auto_close=false 保持界面 | GUI 双方案离屏冒烟 |

