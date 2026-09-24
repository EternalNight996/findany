//! 历史结果重传：把 `filter_result.xlsx` 读回来，**只重传上次失败/未回传的台**，
//! 结果写回原文件（写前自动备份）。
//!
//! 为什么需要：现场常见「回传失败（网络/CLI 异常）」或「先 dry-run 演练、之后要正式回传」，
//! 此时日志可能已被清理，重跑整轮扫描不现实 —— 而 Excel 里已经存着回传所需的全部字段
//! （SN / ProductKeyID / HardwareHash / Baseboard）。
//!
//! 复用：字段提取不用重做（Excel 里就有），回传的组包 / 重试退避 / 超时 / 退出码判定
//! **完全复用 `uploader::run_upload`** —— 不新写一套判定，避免两处口径不一致。

use super::engine::{FilterEvent, FilterHandle, FilterOutcome, FilterSummary};
use super::report;
use super::uploader::{run_upload, UploadProfile};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

/// 后台跑「历史结果重传」（独立模式的执行体）。
///
/// - 递归找 `filter_result.xlsx` → 逐文件读表 → 只挑需重传的行 → 逐台回传 → 写回原表
/// - **每完成一个文件推一行结果**（`FilterEvent::Batch`，复用筛选那套事件，UI 不用新增变体）
/// - 进度节点走 `resume`（`<out_dir>/_resume.json` 记 `done_files`）：中断后能续、跳过已处理文件
pub fn spawn_retry(
    root_dir: String,
    out_dir: String,
    profile: UploadProfile,
    dry_run: bool,
    tx: SyncSender<FilterEvent>,
) -> Arc<FilterHandle> {
    let handle = Arc::new(FilterHandle::default());
    let h = handle.clone();
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let mut summary = FilterSummary::default();
        let files = collect_filter_xlsx(Path::new(&root_dir));
        summary.total = files.len();
        h.total.store(files.len(), Ordering::Relaxed);

        // 断点续跑：这次跳过哪些 xlsx？（上次已处理完的）
        //
        // **追加式记录**（`_resume_retry_done.txt`，一行一个文件）：每个文件只追加一行，O(1)。
        // 以前是把全量 done_files 塞进节点、每完成一个文件就把整份 JSON 重写一遍 ——
        // 一万个 xlsx 就是 O(N²) 的写放大，跑到七八千个时磁盘写入把程序拖到像卡死。
        let done_log = if out_dir.trim().is_empty() {
            String::new()
        } else {
            Path::new(&out_dir).join("_resume_retry_done.txt").to_string_lossy().to_string()
        };
        let mut skip: std::collections::HashSet<String> = read_done_files(&done_log);
        if skip.is_empty() {
            // 老记录兜底：节点里还有全量 done_files 的那种（本版之后不再写）
            if let Some(n) = super::resume::load_node(&out_dir, "retry") {
                skip = n.done_files.into_iter().collect();
            }
        }

        // 进度节点：开始时写一份（重传的「总数」= xlsx 个数）；之后每 20 个文件或跑完刷一次
        let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let files_n = files.len();
        let mut node_done = skip.len();
        let write_node = |done: usize| {
            if out_dir.trim().is_empty() {
                return;
            }
            let n = super::resume::ResumeNode {
                version: 1,
                mode: "retry".into(),
                root_dir: root_dir.clone(),
                out_dir: out_dir.clone(),
                fingerprint: String::new(),
                total: files_n,
                done,
                started_at: started.clone(),
                updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                data_file: String::new(), // 重传结果直接写回 xlsx，不需要旁路数据文件
                done_files: Vec::new(),   // 明细在 _resume_retry_done.txt（追加式，不再塞进节点）
                done_dirs: Vec::new(),    // 重传按文件（每个 xlsx 一个单位），不用文件夹级续跑
            };
            let _ = super::resume::save_node(&out_dir, &n);
        };
        write_node(node_done);

        // 续跑：进度条从「已完成」开始（不然看起来像从头跑）
        h.done.store(skip.len(), Ordering::Relaxed);
        let _ = tx.send(FilterEvent::Log(
            "info".to_string(),
            format!(
                "历史结果重传：共 {} 个结果文件，跳过已完成的 {} 个，本轮处理 {} 个",
                files_n,
                skip.len(),
                files_n.saturating_sub(skip.len())
            ),
        ));

        let mut rows: Vec<Map<String, Value>> = Vec::new();
        let mut done_n = 0usize;
        for f in files.iter() {
            if h.cancel.load(Ordering::Relaxed) {
                break;
            }
            if skip.contains(&norm_path(f)) {
                continue;
            }
            let mut row = Map::new();
            row.insert("file".into(), Value::String(f.clone()));
            // 表格的行索引 / 就地更新都按 `rel_path` 走：缺了它，所有文件会挤在同一行上刷新
            row.insert("rel_path".into(), Value::String(f.clone()));
            row.insert("file_name".into(), Value::String(file_basename(f)));
            row.insert(
                "dir".into(),
                Value::String(Path::new(f).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()),
            );
            let state;
            let mut err = String::new();
            match read_history(f) {
                Ok(sheet) => {
                    // 先按「状态 ∈ 空/失败/dry-run」挑，再按「字段齐不齐」分两组：
                    // 缺 OA3 必需字段的行（非 OA3 判型，如 e-autotest）**不算待重传** ——
                    // 它们传也传不了，直接记「跳过(缺字段)」并写回，免得显示成待重传再报失败。
                    let (ready, skipped_rows) = retry_targets(&profile, &sheet);
                    row.insert("rows_total".into(), Value::Number(sheet.rows.len().into()));
                    row.insert("retry_targets".into(), Value::Number(ready.len().into()));
                    if ready.is_empty() && skipped_rows.is_empty() {
                        state = "无需重传".to_string();
                    } else if ready.is_empty() {
                        // 一个可传的都没有（非 OA3 / 缺字段）：**不写回原文件**，只在界面/日志里说清
                        state = format!("跳过 {} 台（不满足 OA3 回传条件，未写回）", skipped_rows.len());
                        row.insert("skipped".into(), Value::Number(skipped_rows.len().into()));
                        if err.is_empty() {
                            let uniq: std::collections::BTreeSet<&str> =
                                skipped_rows.iter().map(|(_, w)| w.as_str()).collect();
                            err = format!("跳过 {} 台：{}", skipped_rows.len(), uniq.into_iter().collect::<Vec<_>>().join("；"));
                        }
                    } else {
                        // 只写回**真正回传过**的行（跳过的一律不动原文件）
                        let mut updates: Vec<(u32, Map<String, Value>)> = Vec::new();
                        let (mut ok, mut fail, mut conflict, mut dry) = (0usize, 0usize, 0usize, 0usize);
                        for r in sheet.rows.iter().filter(|r| ready.contains(&r.excel_row)) {
                            let fields = retry_one(&profile, r, dry_run, &|_, _| {});
                            // **逐台实时计数**：每回传完一台就更新 handle 的原子，
                            // 界面上的「回传成功 / 冲突 / 失败」运行中就能一秒一变
                            //（以前只累计到结束时的 summary，跑的时候一直显示 0）。
                            match fields.get("_status").and_then(|v| v.as_str()).unwrap_or("") {
                                super::uploader::ST_OK => {
                                    ok += 1;
                                    h.upload_ok.fetch_add(1, Ordering::Relaxed);
                                }
                                super::uploader::ST_CONFLICT => {
                                    conflict += 1;
                                    h.upload_conflict.fetch_add(1, Ordering::Relaxed);
                                }
                                super::uploader::ST_DRY_RUN => {
                                    dry += 1;
                                    h.upload_dry.fetch_add(1, Ordering::Relaxed);
                                }
                                ST_SKIP => {}
                                _ => {
                                    fail += 1;
                                    h.upload_fail.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            updates.push((r.excel_row, fields));
                        }
                        let skip = skipped_rows.len();
                        row.insert("ok".into(), Value::Number(ok.into()));
                        row.insert("conflict".into(), Value::Number(conflict.into()));
                        row.insert("fail".into(), Value::Number(fail.into()));
                        row.insert("skipped".into(), Value::Number(skip.into()));
                        row.insert("dry".into(), Value::Number(dry.into()));
                        summary.upload_ok += ok;
                        summary.upload_conflict += conflict;
                        summary.upload_fail += fail;
                        summary.upload_dry += dry;
                        match write_back(f, &updates) {
                            Ok((_bak, cells)) => {
                                state = if skipped_rows.is_empty() {
                                    format!("完成（写回 {cells} 格）")
                                } else {
                                    // 跳过的行没写回，这里只说清「传了几台、跳过几台」
                                    format!("完成（写回 {cells} 格，另有 {} 台跳过未写回）", skipped_rows.len())
                                }
                            }
                            Err(e) => {
                                state = "写回失败".to_string();
                                err = e.to_string();
                            }
                        }
                        // 跳过的原因也要在界面上看得到（行级原因写在 xlsx 行里，这里给文件级一眼能读的）
                        if !skipped_rows.is_empty() && err.is_empty() {
                            let uniq: std::collections::BTreeSet<&str> =
                                skipped_rows.iter().map(|(_, w)| w.as_str()).collect();
                            err = format!("跳过 {} 台：{}", skipped_rows.len(), uniq.into_iter().collect::<Vec<_>>().join("；"));
                        }
                    }
                }
                Err(e) => {
                    state = "读取失败".to_string();
                    err = e.to_string();
                }
            }
            // 坏文件清单：读不动 / 写不动的文件单独记一份，跑完一眼知道哪些要人工处理
            if state == "读取失败" || state == "写回失败" {
                append_bad_file(&out_dir, f, &state, &err);
            }
            // 每个文件留一行诊断日志：出问题时能直接查「哪个文件、什么状态、待传几台、跳过几台、什么原因」
            row.insert("state".into(), Value::String(state));
            row.insert("error".into(), Value::String(err));
            summary.extracted += 1;
            done_n += 1;
            h.done.fetch_add(1, Ordering::Relaxed);
            h.extracted.fetch_add(1, Ordering::Relaxed);
            // 已完成明细：**追加一行**（O(1)）；节点每 20 个文件才刷一次，避免每文件重写整份 JSON
            append_done_file(&done_log, f);
            node_done = skip.len() + done_n;
            if done_n % 20 == 0 {
                write_node(node_done);
            }
            // **先把结果推给界面**（「启动中」立刻结束、表格开始出数），再写诊断日志 ——
            // 日志是辅助，绝不能挡在结果前面（否则界面停在「启动中」像卡死）。
            let _ = tx.send(FilterEvent::Batch(Box::new(FilterOutcome {
                items: vec![row.clone()],
                summary: summary.clone(),
            })));
            let num = |k: &str| row.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            if done_n == 1 || done_n % 50 == 0 {
                crate::core::app_dir::log_line(
                    "findany-run.log",
                    "info",
                    &format!(
                        "[重传] 进度 {}/{} | {} | 待重传 {} 跳过 {} | {} | {f}",
                        node_done,
                        files_n,
                        row.get("state").and_then(|v| v.as_str()).unwrap_or(""),
                        num("retry_targets"),
                        num("skipped"),
                        {
                            let e = row.get("error").and_then(|v| v.as_str()).unwrap_or("");
                            if e.is_empty() { "-".to_string() } else { e.to_string() }
                        }
                    ),
                );
            }
            rows.push(row);
        }
        // 跑完：清节点（下次点开始不再追问）+ 删掉已完成明细
        if !h.cancel.load(Ordering::Relaxed) {
            write_node(node_done);
            super::resume::clear_node(&out_dir, "retry");
            let _ = std::fs::remove_file(&done_log);
        }
        summary.elapsed = (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0;
        let _ = tx.send(FilterEvent::Done(Box::new(FilterOutcome { items: rows, summary }), String::new()));
    });
    handle
}

/// 「回传状态」列的取值（写回时报错口径与此一致）
pub const ST_UPLOAD_OK: &str = "成功";
pub const ST_UPLOAD_CONFLICT: &str = "冲突(人工)";
pub const ST_UPLOAD_FAIL: &str = "失败";
/// 不满足回传条件的行写进 xlsx「回传状态」的值（原因写在「回传错误」列）
pub const ST_SKIP: &str = "跳过";
pub const ST_UPLOAD_DRY: &str = "dry-run";

/// 一条来自历史 Excel 的数据行
#[derive(Debug, Clone, Default)]
pub struct HistRow {
    /// Excel 行号（1-based；第 1 行是表头，数据从第 2 行起）。写回时按它定位。
    pub excel_row: u32,
    /// 字段名 -> 值（键与内部 Row 一致：sn / product_key_id / hardware_hash / baseboard_product …）
    pub fields: Map<String, Value>,
}

/// 读回来的历史表
#[derive(Debug, Clone, Default)]
pub struct HistSheet {
    /// 读的是哪个 sheet（诊断用；批量重传时不再逐文件展示）
    #[allow(dead_code)]
    pub sheet_name: String,
    /// 字段名 -> Excel 列号（1-based）。写回时另算一份（读写各自独立，不互相依赖）。
    #[allow(dead_code)]
    pub header_col: HashMap<String, u32>,
    pub rows: Vec<HistRow>,
}

/// 上次回传状态（读自「回传状态」列）
pub fn prev_state(row: &HistRow) -> String {
    row.fields.get("upload_state").and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

/// 需要**重新回传**的行：**只看「OA3 必需字段是否齐全」，不看上次回传状态、也不看判型名**。
///
/// 判型只是提示：`e-autotest` 的产物同样可能带齐 SN / ProductKeyID / HardwareHash / Baseboard
/// （实测就该传）。判型写着 `etest(OA3)` 但字段空的行，反而传不了 —— 所以门槛是**字段**，不是判型。
/// 字段齐全 → 重新回传（服务端对重复上报返回 duplicate_accepted）；缺字段 → 跳过并写清缺哪些。
///
/// 返回 `(要重传的行号, [(跳过的行号, 原因)])`
pub fn retry_targets(profile: &UploadProfile, sheet: &HistSheet) -> (Vec<u32>, Vec<(u32, String)>) {
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for r in sheet.rows.iter() {
        let payload = super::uploader::build_payload(&row_fields(r), &profile.field_map);
        let miss = super::uploader::missing_fields(&payload);
        if miss.is_empty() {
            ready.push(r.excel_row);
        } else {
            let dt = r.fields.get("detected_type").and_then(|v| v.as_str()).unwrap_or("").trim();
            skipped.push((
                r.excel_row,
                if dt.is_empty() {
                    format!("缺字段：{}", miss.join(","))
                } else {
                    format!("缺字段：{}（判型 {}）", miss.join(","), dt)
                },
            ));
        }
    }
    (ready, skipped)
}

/// 坏文件清单（`<out_dir>/_bad_files.txt`）：读不动/写不动的文件，一行一个
fn append_bad_file(out_dir: &str, file: &str, state: &str, err: &str) {
    use std::io::Write;
    if out_dir.trim().is_empty() {
        return;
    }
    let p = Path::new(out_dir).join("_bad_files.txt");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{state}\t{file}\t{err}");
    }
}

/// 路径规范化：Windows 路径**大小写不敏感、正/反斜杠等价**。
/// 比较「已完成」集合时必须规范化 —— 否则用户把 root_dir 写成 `g:/1dgl/...` 或大小写不同时，
/// 跳过集全部失效，点了「继续上次」却**从头重跑**整批（真实踩到的坑）。
fn norm_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// 读「已完成文件」追加记录（一行一个绝对路径）。文件不存在 = 空表。
fn read_done_files(path: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if path.is_empty() {
        return out;
    }
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let l = line.trim();
            if !l.is_empty() {
                out.insert(norm_path(l));
            }
        }
    }
    out
}

/// 追加一行「已完成文件」：**O(1)**，取代「每完成一个文件就重写整份 JSON」的写放大。
/// 写失败不影响本次运行（只是那个文件下次续跑时会重跑一遍）。
fn append_done_file(path: &str, file: &str) {
    use std::io::Write;
    if path.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{file}");
    }
}

/// 每行实际用于组包的字段（与 `retry_one` 同一口径，预检和实传不会各说一套）
fn row_fields(row: &HistRow) -> Map<String, Value> {
    row.fields
        .get("fields")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_else(|| row.fields.clone())
}

/// **递归**收集目录下所有 findany 产出的 `filter_result.xlsx`。
///
/// 场景：输出根下是一堆批次目录（`<out>/2026-09-23_10-00/filter_result.xlsx`），
/// 用户选输出根就能一次把历次批次全重传一遍，不用一个个点进去。
///
/// 规则：
/// - 文件名**精确**等于 `filter_result.xlsx`（忽略大小写）
/// - 跳过 `.bak-时间戳` 备份（写回时留的旧副本，再传一次没意义）
/// - 跳过 `.` 开头的隐藏目录（与遍历口径一致）
/// - 传进来是文件本身也接受（容错）
pub fn collect_filter_xlsx(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if root.is_file() {
        if root
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("filter_result.xlsx"))
            .unwrap_or(false)
        {
            out.push(root.to_string_lossy().to_string());
        }
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if name.starts_with('.') {
                    continue;
                }
                stack.push(p);
            } else if name.eq_ignore_ascii_case("filter_result.xlsx") {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    out.sort(); // 按路径排序：重传顺序可预期（旧批次在前）
    out
}

/// 取文件名（错误提示里用，避免刷一整条长路径）
pub fn file_basename(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

/// 读表头 → 字段名 / 列号（导出与读回用同一张列模板，列序变了也能对上）
fn header_map(ws: &umya_spreadsheet::Worksheet) -> HashMap<String, u32> {
    let cols = report::detail_cols();
    let mut out = HashMap::new();
    let max_col = ws.get_highest_column();
    for c in 1..=max_col {
        let h = ws.get_value((c, 1u32)).to_string();
        let h = h.trim();
        if h.is_empty() {
            continue;
        }
        if let Some((_, key)) = cols.iter().find(|(title, _)| *title == h) {
            out.insert((*key).to_string(), c);
        }
    }
    out
}

/// 读取历史 `filter_result.xlsx`（优先「明细」sheet，没有就用第一个）
pub fn read_history(path: &str) -> anyhow::Result<HistSheet> {
    let book = umya_spreadsheet::reader::xlsx::read(Path::new(path))
        .map_err(|e| anyhow::anyhow!("读取 xlsx 失败：{e:?}"))?;
    let ws = book
        .get_sheet_by_name("明细")
        .or_else(|| book.get_sheet(&0))
        .ok_or_else(|| anyhow::anyhow!("xlsx 里没有可读的 sheet"))?;
    let sheet_name = ws.get_name().to_string();
    let header_col = header_map(ws);
    anyhow::ensure!(
        !header_col.is_empty(),
        "表头一行没认出任何已知列 —— 请选 findany 产出的 filter_result.xlsx（「明细」表）"
    );
    let max_row = ws.get_highest_row();
    let mut rows = Vec::new();
    for r in 2..=max_row {
        let mut fields = Map::new();
        let mut all_empty = true;
        for (key, c) in &header_col {
            let v = ws.get_value((*c, r)).to_string();
            if !v.trim().is_empty() {
                all_empty = false;
            }
            fields.insert(key.clone(), Value::String(v));
        }
        if all_empty {
            continue; // 空行跳过（Excel 常见的尾部空行）
        }
        rows.push(HistRow { excel_row: r, fields });
    }
    Ok(HistSheet { sheet_name, header_col, rows })
}

/// 重传一台：**复用统一回传**（组包/重试/超时/判定全同一套），返回写回该行的字段
pub fn retry_one(profile: &UploadProfile, row: &HistRow, dry_run: bool, on_log: &dyn Fn(&str, &str)) -> Map<String, Value> {
    let fields = row_fields(row);
    let res = run_upload(profile, &fields, dry_run, on_log, None);
    // 缺 OA3 必需字段（product_key_id / hardware_hash / baseboard_product）= 这批日志本来就不是
    // OA3 判型（例如 e-autotest 提取出来只有 ProductKey）—— 重传再多次也缺，记成「跳过(缺字段)」，
    // 别写成「失败」：否则整批看着像回传全挂了（用户就是这么被误导的）。
    let no_fields = res.status == super::uploader::ST_FAIL && res.error.starts_with("字段不全");
    let mut out = Map::new();
    let state = if no_fields {
        ST_SKIP
    } else {
        match res.status.as_str() {
            super::uploader::ST_OK => ST_UPLOAD_OK,
            super::uploader::ST_CONFLICT => ST_UPLOAD_CONFLICT,
            super::uploader::ST_FAIL => ST_UPLOAD_FAIL,
            super::uploader::ST_DRY_RUN => ST_UPLOAD_DRY,
            other => other,
        }
    };
    out.insert("upload_state".into(), Value::String(state.into()));
    out.insert("upload_code".into(), Value::String(res.exit_code.map(|c| c.to_string()).unwrap_or_default()));
    out.insert("request_id".into(), Value::String(res.request_id.clone()));
    out.insert("resp_status".into(), Value::String(res.resp_status.clone()));
    out.insert("upload_error".into(), Value::String(res.error.clone()));
    out.insert("upload_attempts".into(), Value::String(if res.attempts > 0 { res.attempts.to_string() } else { String::new() }));
    out.insert("upload_elapsed".into(), Value::String(res.elapsed.to_string()));
    // 供调用方做统计（不进 Excel）
    out.insert(
        "_status".into(),
        Value::String(if no_fields { ST_SKIP.into() } else { res.status.clone() }),
    );
    out
}

/// 写回原 xlsx：**先备份**（写回是不可逆操作），再按 (Excel 行号, 字段名) 更新单元格。
/// 返回 (备份文件路径, 更新单元格数)。
pub fn write_back(path: &str, updates: &[(u32, Map<String, Value>)]) -> anyhow::Result<(String, usize)> {
    if updates.is_empty() {
        return Ok((String::new(), 0));
    }
    // 0) 先探一次可写：被 Excel / 杀软占着时当场说清，别等到写了一半才发现
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("目标文件不可写（可能正被 Excel 打开或杀软锁定）：{e}"))?;
    // 1) 备份：文件名带时间戳，便于回滚（Excel 被 umya 重写后格式会变，原文件必须有副本）
    let bak = format!("{}.bak-{}", path, chrono::Local::now().format("%Y%m%d-%H%M%S"));
    std::fs::copy(path, &bak).map_err(|e| anyhow::anyhow!("备份原文件失败（{bak}）：{e}"))?;

    // 2) 打开原表（先判断再取可变引用：`or_else` 闭包里再借 book 会与第一个可变借用冲突）
    let mut book = umya_spreadsheet::reader::xlsx::read(Path::new(path))
        .map_err(|e| anyhow::anyhow!("写回时读取 xlsx 失败：{e:?}"))?;
    let ws = if book.get_sheet_by_name("明细").is_some() {
        book.get_sheet_by_name_mut("明细").expect("上面刚判断过存在")
    } else {
        book.get_sheet_mut(&0).ok_or_else(|| anyhow::anyhow!("xlsx 里没有可写的 sheet"))?
    };
    let header_col = header_map(ws);

    // 3) 逐格更新（只动回传结果那几列，其余列原样保留）
    let mut n = 0usize;
    for (row_no, fields) in updates {
        for (key, val) in fields {
            if key.starts_with('_') {
                continue; // 内部标记（如 _status）不落表
            }
            let Some(c) = header_col.get(key) else { continue };
            let s = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            ws.get_cell_mut((*c, *row_no)).set_value(s.as_str());
            n += 1;
        }
    }

    // 4) **原子写**：先写到同目录的临时文件，校验后再 rename 覆盖原文件。
    //    中途失败（断电 / 被杀 / 磁盘满）原文件仍是完整的，不会留下半截 xlsx。
    let tmp = format!("{path}.tmp-{}", std::process::id());
    umya_spreadsheet::writer::xlsx::write(&book, Path::new(&tmp))
        .map_err(|e| anyhow::anyhow!("写回 xlsx 失败（临时文件 {tmp}）：{e:?}"))?;
    if std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("写回校验失败：临时文件是空的，原文件未改动"));
    }
    // Windows 上 std::fs::rename 走 MoveFileEx(REPLACE_EXISTING)：目标存在也是原子替换
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("写回替换失败（原文件未改动，备份在 {bak}）：{e}"));
    }
    Ok((bak, n))
}

/// 重传结论（界面 R 标记与 `--auto` 共用一套口径）：
/// 找不到结果文件、有回传失败或冲突 = FAIL；整目录都无需重传算正常（没有待重传的行不是异常）。
pub fn retry_verdict(summary: &FilterSummary, dry_run: bool) -> (String, bool) {
    let ran = summary.upload_ok + summary.upload_conflict + summary.upload_fail;
    if summary.total == 0 || summary.extracted == 0 {
        return (
            format!("没有可重传的 filter_result.xlsx（候选 {}，处理 {}）", summary.total, summary.extracted),
            false,
        );
    }
    let tail = if dry_run {
        format!("（dry-run {}）", summary.upload_dry)
    } else if ran == 0 {
        "，本次没有待重传的行".to_string()
    } else {
        String::new()
    };
    (
        format!(
            "重传 {} 个结果文件：回传 成功 {}/冲突 {}/失败 {}{}，耗时 {}s",
            summary.extracted, summary.upload_ok, summary.upload_conflict, summary.upload_fail, tail, summary.elapsed
        ),
        summary.upload_fail == 0 && summary.upload_conflict == 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sm(total: usize, extracted: usize, ok: usize, conflict: usize, fail: usize) -> FilterSummary {
        FilterSummary { total, extracted, upload_ok: ok, upload_conflict: conflict, upload_fail: fail, ..Default::default() }
    }

    #[test]
    fn retry_verdict_fails_on_empty_fail_and_conflict() {
        assert!(!retry_verdict(&sm(0, 0, 0, 0, 0), false).1, "一个结果文件都没找到必须 FAIL");
        assert!(!retry_verdict(&sm(3, 3, 5, 0, 1), false).1, "有回传失败必须 FAIL");
        assert!(!retry_verdict(&sm(3, 3, 4, 1, 0), false).1, "冲突(人工)要人工介入，与界面拦截同口径 -> FAIL");
        let (txt, ok) = retry_verdict(&sm(2, 2, 7, 0, 0), false);
        assert!(ok, "全部成功 -> PASS：{txt}");
        assert!(retry_verdict(&sm(2, 2, 0, 0, 0), false).1, "整目录都无需重传 -> PASS（不是异常）");
        assert!(txt.contains("成功 7"), "结论里要能看到成功台数：{txt}");
    }

    /// 已完成明细是追加式文件（一行一个），不再每文件重写整份节点 JSON
    #[test]
    fn done_log_append_and_read() {
        let p = std::env::temp_dir().join("findany-retry-done-log.txt");
        let _ = std::fs::remove_file(&p);
        let s = p.to_string_lossy().to_string();
        append_done_file(&s, "D:/a/filter_result.xlsx");
        append_done_file(&s, "D:/b/filter_result.xlsx");
        let got = read_done_files(&s);
        assert_eq!(got.len(), 2, "两行都要读回来");
        assert!(got.contains(&norm_path("D:/a/filter_result.xlsx")), "查也是按规范化路径");
        // 大小写 / 斜杠方向不同也必须命中（Windows 语义）
        assert!(got.contains(&norm_path("d:\\A\\FILTER_RESULT.XLSX")));
        let _ = std::fs::remove_file(&p);
    }

    /// 口径：只看 OA3 必需字段是否齐全 —— 不看上次状态，也不看判型名
    #[test]
    fn retry_targets_by_fields_only() {
        let row_of = |excel_row: u32, dt: &str, state: &str, full: bool| {
            let mut f = Map::new();
            f.insert("detected_type".into(), Value::String(dt.into()));
            f.insert("upload_state".into(), Value::String(state.into()));
            f.insert("sn".into(), Value::String("SN".into()));
            f.insert("product_key_id".into(), Value::String("PK".into()));
            f.insert("baseboard_product".into(), Value::String("B".into()));
            if full {
                f.insert("hardware_hash".into(), Value::String("H".into()));
            }
            HistRow { excel_row, fields: f }
        };
        let sheet = HistSheet {
            sheet_name: "明细".into(),
            header_col: HashMap::new(),
            rows: vec![
                row_of(2, "etest(OA3)", "成功", true),   // 上次成功 → 仍要重传
                row_of(3, "etest(OA3)", "冲突(人工)", true),
                row_of(4, "e-autotest", "", true),       // **判型是 e-autotest，但字段齐 → 也要传**
                row_of(5, "etest(OA3)", "", false),      // 缺 hardware_hash → 跳过
            ],
        };
        let (ready, skipped) = retry_targets(&UploadProfile::default(), &sheet);
        assert_eq!(ready, vec![2, 3, 4], "字段齐就传：不看旧状态、不看判型");
        assert_eq!(skipped.len(), 1, "缺字段的那行才跳过");
        assert_eq!(skipped[0].0, 5);
        assert!(skipped[0].1.contains("缺字段"), "{skipped:?}");
    }
}
