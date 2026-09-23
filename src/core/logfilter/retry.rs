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
        let skip: std::collections::HashSet<String> =
            super::resume::load_node(&out_dir).map(|n| n.done_files.into_iter().collect()).unwrap_or_default();

        // 进度节点：开始时写一份（重传的「总数」= xlsx 个数）
        let started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut done_files: Vec<String> = skip.iter().cloned().collect();
        let files_n = files.len();
        let write_node = |done: usize, done_files: &[String]| {
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
                done_files: done_files.to_vec(),
            };
            let _ = super::resume::save_node(&out_dir, &n);
        };
        write_node(skip.len(), &done_files);

        let mut rows: Vec<Map<String, Value>> = Vec::new();
        let mut done_n = 0usize;
        for f in files.iter() {
            if h.cancel.load(Ordering::Relaxed) {
                break;
            }
            if skip.contains(f) {
                continue;
            }
            let mut row = Map::new();
            row.insert("file".into(), Value::String(f.clone()));
            row.insert("file_name".into(), Value::String(file_basename(f)));
            row.insert(
                "dir".into(),
                Value::String(Path::new(f).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default()),
            );
            let mut state;
            let mut err = String::new();
            match read_history(f) {
                Ok(sheet) => {
                    let targets = retry_targets(&sheet);
                    row.insert("rows_total".into(), Value::Number(sheet.rows.len().into()));
                    row.insert("retry_targets".into(), Value::Number(targets.len().into()));
                    if targets.is_empty() {
                        state = "无需重传".to_string();
                    } else {
                        let mut updates = Vec::new();
                        let (mut ok, mut fail, mut conflict, mut dry) = (0usize, 0usize, 0usize, 0usize);
                        for r in sheet.rows.iter().filter(|r| targets.contains(&r.excel_row)) {
                            let fields = retry_one(&profile, r, dry_run, &|_, _| {});
                            match fields.get("_status").and_then(|v| v.as_str()).unwrap_or("") {
                                super::uploader::ST_OK => ok += 1,
                                super::uploader::ST_CONFLICT => conflict += 1,
                                super::uploader::ST_DRY_RUN => dry += 1,
                                _ => fail += 1,
                            }
                            updates.push((r.excel_row, fields));
                        }
                        row.insert("ok".into(), Value::Number(ok.into()));
                        row.insert("conflict".into(), Value::Number(conflict.into()));
                        row.insert("fail".into(), Value::Number(fail.into()));
                        row.insert("dry".into(), Value::Number(dry.into()));
                        summary.upload_ok += ok;
                        summary.upload_conflict += conflict;
                        summary.upload_fail += fail;
                        summary.upload_dry += dry;
                        match write_back(f, &updates) {
                            Ok((_bak, cells)) => state = format!("完成（写回 {cells} 格）"),
                            Err(e) => {
                                state = "写回失败".to_string();
                                err = e.to_string();
                            }
                        }
                    }
                }
                Err(e) => {
                    state = "读取失败".to_string();
                    err = e.to_string();
                }
            }
            row.insert("state".into(), Value::String(state));
            row.insert("error".into(), Value::String(err));
            summary.extracted += 1;
            done_n += 1;
            h.done.fetch_add(1, Ordering::Relaxed);
            h.extracted.fetch_add(1, Ordering::Relaxed);
            done_files.push(f.clone());
            // 每完成一个文件：刷新节点 + 推一行给界面
            write_node(skip.len() + done_n, &done_files);
            let _ = tx.send(FilterEvent::Batch(Box::new(FilterOutcome {
                items: vec![row.clone()],
                summary: summary.clone(),
            })));
            rows.push(row);
        }
        // 跑完：清节点（下次点开始不再追问）
        if !h.cancel.load(Ordering::Relaxed) {
            super::resume::clear_node(&out_dir);
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

/// 需要重传的判定：**空 / 失败 / dry-run**。
/// 「冲突(人工)」与「成功」不自动重传 —— 冲突要人工介入，成功不该重复上报。
pub fn needs_retry(state: &str) -> bool {
    matches!(state, "" | ST_UPLOAD_FAIL | ST_UPLOAD_DRY)
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

/// 读回来的行里，需要重传的 Excel 行号（保持原表顺序）
pub fn retry_targets(sheet: &HistSheet) -> Vec<u32> {
    sheet
        .rows
        .iter()
        .filter(|r| needs_retry(&prev_state(r)))
        .map(|r| r.excel_row)
        .collect()
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
    let fields = row.fields.get("fields").and_then(|v| v.as_object()).cloned().unwrap_or_else(|| {
        // 历史 Excel 没有嵌套 fields，直接用行内字段（run_upload 只按 field_map 取源字段）
        row.fields.clone()
    });
    let res = run_upload(profile, &fields, dry_run, on_log, None);
    let mut out = Map::new();
    let state = match res.status.as_str() {
        super::uploader::ST_OK => ST_UPLOAD_OK,
        super::uploader::ST_CONFLICT => ST_UPLOAD_CONFLICT,
        super::uploader::ST_FAIL => ST_UPLOAD_FAIL,
        super::uploader::ST_DRY_RUN => ST_UPLOAD_DRY,
        other => other,
    };
    out.insert("upload_state".into(), Value::String(state.into()));
    out.insert("upload_code".into(), Value::String(res.exit_code.map(|c| c.to_string()).unwrap_or_default()));
    out.insert("request_id".into(), Value::String(res.request_id.clone()));
    out.insert("resp_status".into(), Value::String(res.resp_status.clone()));
    out.insert("upload_error".into(), Value::String(res.error.clone()));
    out.insert("upload_attempts".into(), Value::String(if res.attempts > 0 { res.attempts.to_string() } else { String::new() }));
    out.insert("upload_elapsed".into(), Value::String(res.elapsed.to_string()));
    // 供调用方做统计（不进 Excel）
    out.insert("_status".into(), Value::String(res.status.clone()));
    out
}

/// 写回原 xlsx：**先备份**（写回是不可逆操作），再按 (Excel 行号, 字段名) 更新单元格。
/// 返回 (备份文件路径, 更新单元格数)。
pub fn write_back(path: &str, updates: &[(u32, Map<String, Value>)]) -> anyhow::Result<(String, usize)> {
    if updates.is_empty() {
        return Ok((String::new(), 0));
    }
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

    // 4) 保存回原文件
    umya_spreadsheet::writer::xlsx::write(&book, Path::new(path))
        .map_err(|e| anyhow::anyhow!("写回 xlsx 失败：{e:?}"))?;
    Ok((bak, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str) -> HistRow {
        let mut fields = Map::new();
        fields.insert("upload_state".into(), Value::String(state.into()));
        HistRow { excel_row: 2, fields }
    }

    #[test]
    fn needs_retry_covers_failed_empty_and_dry_run() {
        assert!(needs_retry(""), "空（从没传过）必须重传");
        assert!(needs_retry("失败"));
        assert!(needs_retry("dry-run"));
        assert!(!needs_retry("成功"), "成功不该重复上报");
        assert!(!needs_retry("冲突(人工)"), "冲突要人工介入，不能自动重传");
    }

    #[test]
    fn retry_targets_keeps_table_order() {
        let sheet = HistSheet {
            sheet_name: "明细".into(),
            header_col: HashMap::new(),
            rows: vec![
                HistRow { excel_row: 2, ..row("成功") },
                HistRow { excel_row: 3, ..row("失败") },
                HistRow { excel_row: 4, ..row("") },
                HistRow { excel_row: 5, ..row("冲突(人工)") },
                HistRow { excel_row: 6, ..row("dry-run") },
            ],
        };
        assert_eq!(retry_targets(&sheet), vec![3, 4, 6]);
    }
}
