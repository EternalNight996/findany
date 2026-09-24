//! 输出层：批次目录 + Excel 明细导出（含摘要） + 命中文件落盘。
//!
//! 注意：通用扫描的产物导出已并入统一管道（engine::export_products），本文件里
//! `export_excel` / `export_csv` / `copy_hits` 是旧扫描导出路径的残留（已无调用方），
//! 只有 `make_batch_dir` / `column_letter` 仍被统一管道使用。

use crate::core::config::SearchConfig;
use crate::core::scanner::{clean_cell, ScanItem, ScanSummary};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BatchDir {
    pub path: String,
    #[allow(dead_code)]
    pub name: String,
}

/// out_root 下建「YYYY-MM-DD_HH-MM-SS」批次目录，同秒撞车才自动加序号。
///
/// **必须精确到秒**：以前只到分钟，同分钟内靠 `_2 / _3 / …` **逐个 exists 探测**。
/// 分批模式下（按子目录分批，动辄几千上万批）同一分钟能跑几百批，探测次数线性增长、
/// 总代价 O(N²) —— 在慢盘/网络盘上就是「越跑越慢直到像卡死」。
/// 精确到秒之后同秒撞车极少，绝大多数批次一次探测就过。
pub fn make_batch_dir(out_root: &str) -> std::io::Result<BatchDir> {
    let name = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    // 先把输出根解析成绝对路径：相对路径（含 "."）与进程 cwd 无关地落到预期位置，
    // 也避免 Path::join 在 "." 下拼出 "./xxx" 这类依赖 cwd 的相对批次目录
    let root = crate::core::scanner::abs_path(Path::new(if out_root.is_empty() { "." } else { out_root }));
    let base = root.join(&name);
    let mut path = base.clone();
    let mut n = 2;
    while path.exists() {
        path = PathBuf::from(format!("{}_{}", base.to_string_lossy(), n));
        n += 1;
    }
    std::fs::create_dir_all(&path)?;
    Ok(BatchDir { path: path.to_string_lossy().to_string(), name })
}

#[allow(dead_code)]
fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_lowercase(),
        None => String::new(),
    }
}

/// 导出 Excel 明细 + 摘要，返回实际生成文件路径（对齐 Python 列序与摘要项）。
#[allow(dead_code)]
pub fn export_excel(
    path: &str,
    items: &[ScanItem],
    summary: &ScanSummary,
    cfg: &SearchConfig,
) -> anyhow::Result<String> {
    use umya_spreadsheet::{Alignment, Color, Font, HorizontalAlignmentValues, VerticalAlignmentValues};

    let rows: Vec<&ScanItem> = items.iter().filter(|i| i.hit || cfg.record_miss).collect();
    let headers = [
        "序号", "相对路径", "目录", "扩展名", "包含状态", "命中行号", "命中行内容", "匹配计数", "大小",
        "修改时间", "编码", "绝对路径",
    ];

    let mut book = umya_spreadsheet::new_file();
    {
        let ws = book.get_sheet_mut(&0).unwrap();
        ws.set_name("明细");
        for (i, h) in headers.iter().enumerate() {
            let c = ws.get_cell_mut(((i + 1) as u32, 1u32));
            c.set_value(*h);
            let mut font = Font::default();
            font.set_bold(true);
            let mut white = Color::default();
            white.set_argb("FFFFFFFF");
            font.set_color(white);
            let mut align = Alignment::default();
            align.set_horizontal(HorizontalAlignmentValues::Center);
            align.set_vertical(VerticalAlignmentValues::Center);
            c.get_style_mut().set_font(font).set_alignment(align).set_background_color_solid("FF4472C4");
        }
        for (idx, it) in rows.iter().enumerate() {
            let r = (idx + 2) as u32;
            let state = if it.hit { "命中" } else { "未命中" };
            let lines = if it.hit {
                it.hit_lines.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
            } else {
                String::new()
            };
            let vals: [String; 12] = [
                (idx + 1).to_string(),
                clean_cell(&it.rel_path),
                clean_cell(&it.dir_name),
                ext_of(&it.filename),
                state.to_string(),
                lines,
                if it.hit { clean_cell(&it.hit_line_text) } else { String::new() },
                (if it.hit { it.hit_count } else { 0 }).to_string(),
                it.size_str(),
                it.mtime_str(),
                it.encoding.clone(),
                clean_cell(&it.abs_path),
            ];
            for (i, v) in vals.iter().enumerate() {
                ws.get_cell_mut(((i + 1) as u32, r)).set_value(v.as_str());
            }
        }
        let widths = [6.0, 46.0, 26.0, 10.0, 10.0, 16.0, 60.0, 10.0, 10.0, 20.0, 14.0, 50.0];
        for (i, w) in widths.iter().enumerate() {
            let letter = column_letter(i + 1);
            ws.get_column_dimension_mut(&letter).set_width(*w);
        }
        // 冻结表头（对齐 openpyxl freeze_panes = "A2"：0 列 + 首行）
        {
            let views = ws.get_sheet_views_mut();
            if views.get_sheet_view_list().is_empty() {
                views.add_sheet_view_list_mut(umya_spreadsheet::SheetView::default());
            }
            if let Some(sv) = views.get_sheet_view_list_mut().first_mut() {
                let mut pane = umya_spreadsheet::Pane::default();
                let mut top_left = umya_spreadsheet::Coordinate::default();
                top_left.set_coordinate("A2");
                pane.set_top_left_cell(top_left);
                pane.set_state(umya_spreadsheet::PaneStateValues::Frozen);
                pane.set_active_pane(umya_spreadsheet::PaneValues::BottomLeft);
                pane.set_vertical_split(1.0);
                sv.set_pane(pane);
            }
        }

        let ws2 = book.new_sheet("摘要").map_err(|e| anyhow::anyhow!("摘要 sheet: {e:?}"))?;
        let mut bold = Font::default();
        bold.set_bold(true);
        ws2.get_cell_mut((1u32, 1u32)).set_value("项目");
        ws2.get_cell_mut((1u32, 1u32)).get_style_mut().set_font(bold.clone());
        ws2.get_cell_mut((2u32, 1u32)).set_value("内容");
        ws2.get_cell_mut((2u32, 1u32)).get_style_mut().set_font(bold);
        let mode = if cfg.mode == "inc" { "包含" } else { "不包含" };
        let summary_rows: Vec<(&str, String)> = vec![
            ("扫描目录", cfg.root_dir.clone()),
            ("关键字", cfg.keyword.clone()),
            ("匹配模式", mode.to_string()),
            ("并发线程", cfg.threads.to_string()),
            ("扩展名过滤", cfg.extensions.join(", ")),
            ("编码", cfg.encoding.clone()),
            ("大小写敏感", if cfg.case_sensitive { "是" } else { "否" }.to_string()),
            ("文件总数", summary.total_files.to_string()),
            ("已扫描文件", summary.scanned.to_string()),
            ("命中文件", summary.hit.to_string()),
            ("未命中文件", summary.miss.to_string()),
            ("跳过文件", summary.skipped.to_string()),
            ("耗时(秒)", format!("{}", summary.elapsed)),
            ("导出时间", chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            ("Excel 文件", Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
        ];
        for (i, (k, v)) in summary_rows.iter().enumerate() {
            let r = (i + 2) as u32;
            ws2.get_cell_mut((1u32, r)).set_value(*k);
            ws2.get_cell_mut((2u32, r)).set_value(v.as_str());
        }
        ws2.get_column_dimension_mut("A").set_width(16.0);
        ws2.get_column_dimension_mut("B").set_width(70.0);
    }
    umya_spreadsheet::writer::xlsx::write(&book, path)
        .map_err(|e| anyhow::anyhow!("写 Excel 失败: {e:?}"))?;
    Ok(path.to_string())
}

/// 导出为 CSV（utf-8-sig，带 BOM）——Excel 写出失败时的降级路径。
#[allow(dead_code)]
pub fn export_csv(
    path: &str,
    items: &[ScanItem],
    cfg: &SearchConfig,
) -> anyhow::Result<String> {
    let rows: Vec<&ScanItem> = items.iter().filter(|i| i.hit || cfg.record_miss).collect();
    let headers = [
        "序号", "相对路径", "目录", "扩展名", "包含状态", "命中行号", "命中行内容", "匹配计数", "大小",
        "修改时间", "编码", "绝对路径",
    ];
    let mut out = String::from("\u{feff}");
    let mut push_row = |cells: Vec<String>| {
        let line = cells
            .iter()
            .map(|c| {
                let c = clean_cell(c);
                if c.contains(',') || c.contains('"') || c.contains('\n') || c.contains('\r') {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    };
    push_row(headers.iter().map(|s| s.to_string()).collect());
    for (idx, it) in rows.iter().enumerate() {
        push_row(vec![
            (idx + 1).to_string(),
            it.rel_path.clone(),
            it.dir_name.clone(),
            ext_of(&it.filename),
            if it.hit { "命中" } else { "未命中" }.to_string(),
            if it.hit { it.hit_lines.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",") } else { String::new() },
            if it.hit { it.hit_line_text.clone() } else { String::new() },
            (if it.hit { it.hit_count } else { 0 }).to_string(),
            it.size_str(),
            it.mtime_str(),
            it.encoding.clone(),
            it.abs_path.clone(),
        ]);
    }
    let csv_path = Path::new(path).with_extension("csv");
    std::fs::write(&csv_path, out)?;
    Ok(csv_path.to_string_lossy().to_string())
}

/// 把命中的文件按「目录名/文件名」落盘到批次目录，返回复制数（同名自动加序号）。
#[allow(dead_code)]
pub fn copy_hits(items: &[ScanItem], batch_dir: &str, _mode: &str) -> usize {
    let mut copied = 0usize;
    for it in items.iter().filter(|i| i.hit) {
        let target_dir = Path::new(batch_dir).join(it.dir_name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if std::fs::create_dir_all(&target_dir).is_err() {
            continue;
        }
        let mut dst = target_dir.join(&it.filename);
        if dst.exists() {
            let stem = Path::new(&it.filename).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ext = Path::new(&it.filename).extension().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let mut n = 2;
            while dst.exists() {
                let name = if ext.is_empty() { format!("{stem}_{n}") } else { format!("{stem}_{n}.{ext}") };
                dst = target_dir.join(name);
                n += 1;
            }
        }
        if std::fs::copy(&it.abs_path, &dst).is_ok() {
            copied += 1;
        }
    }
    copied
}

/// 0-based 列号 → Excel 列字母（1→A, 27→AA）。
pub fn column_letter(mut n: usize) -> String {
    let mut s = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        s.push((b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}
