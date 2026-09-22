//! 筛选批次产物：filter_result.xlsx（CSV 降级）+ upload-result.csv 审计 + 命中日志留存
//! （对齐 sonar/logfilter/report.py）。
//!
//! 审计文件不落 SecretKey / HardwareHash / payload（delivery-sop.md 第 6 条）。

use crate::core::scanner::clean_cell;
use serde_json::{Map, Value};
use std::path::Path;
use umya_spreadsheet::{Alignment, Color, Font, HorizontalAlignmentValues, VerticalAlignmentValues};

/// 行数据（对齐 Python 的 dict）
pub type Row = Map<String, Value>;

fn s(row: &Row, key: &str) -> String {
    // `extract_state` 是**派生列**：由 extract_ok 当场算出，不再存进每行的 Map。
    // 旧实现会在导出前把 rows 整体 clone 一份再逐行 insert 这列（5000 行 × 13KB ≈ 65MB 峰值），
    // 只为加一个布尔派生列，不划算。
    if key == "extract_state" {
        return match row.get("extract_ok").and_then(|v| v.as_bool()) {
            Some(true) => "成功".to_string(),
            _ => "失败".to_string(),
        };
    }
    match row.get(key) {
        Some(Value::String(v)) => v.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

// 统一模板：36 列固定集合，按 6 组逻辑排序（识别→设备→网络→OA3→原始→结果）。
// 导出时「整列全空自动隐藏」——模板统一，视图按批次自适应。
fn detail_cols() -> Vec<(&'static str, &'static str)> {
    vec![
        // 识别
        ("序号", "idx"), ("文件", "log_file"), ("目录", "dir_name"), ("相对路径", "rel_path"),
        ("工位", "station"), ("判型", "detected_type"),
        // 设备
        ("SN", "sn"), ("生产编号", "production_num"), ("系统SN", "system_sn"), ("板卡SN", "board_sn"),
        ("UUID", "uuid"), ("BIOS版本", "bios_version"), ("OS激活码", "os_key"),
        // 网络
        ("有线MAC", "lan"), ("无线MAC", "wifilan"), ("蓝牙MAC", "bluetooth"),
        // OA3
        ("OA3结果", "oa3_result"), ("ProductKeyID", "product_key_id"), ("PKState", "product_key_state"),
        ("ProductKey", "product_key"), ("Hash长度", "hardware_hash_len"), ("Hash SHA-256", "hardware_hash_sha256"),
        ("HardwareHash", "hardware_hash"),
        ("注入开始", "inject_start_at"), ("注入结束", "inject_end_at"),
        ("Baseboard", "baseboard_product"), ("批次号", "mo_lot_no"), ("工位任务", "task_tag"),
        // 原始
        ("JSON状态", "json_state"), ("JSON res_value", "json_res_value"), ("项目版本", "project_version"),
        // 结果
        ("提取状态", "extract_state"),
        ("回传状态", "upload_state"), ("退出码", "upload_code"), ("request_id", "request_id"),
        ("回传错误", "upload_error"),
    ]
}

/// 固定宽度偏好（未列出的按内容自适应，上限 40）
fn col_width_fixed(key: &str) -> Option<f64> {
    Some(match key {
        "idx" => 6.0,
        "log_file" => 42.0,
        "dir_name" => 10.0,
        "rel_path" => 46.0,
        "detected_type" => 14.0,
        "sn" => 34.0,
        "system_sn" => 34.0,
        "production_num" => 34.0,
        "uuid" => 38.0,
        "os_key" => 30.0,
        "hardware_hash_sha256" => 20.0,
        "hardware_hash" => 20.0,
        "product_key" => 30.0,
        "request_id" => 22.0,
        "upload_error" => 40.0,
        _ => return None,
    })
}

fn col_width(key: &str, header: &str, samples: &[String]) -> f64 {
    if let Some(w) = col_width_fixed(key) {
        return w;
    }
    let mut longest = header.chars().count();
    for s in samples.iter().take(80) {
        longest = longest.max(s.chars().count());
    }
    ((longest + 2) as f64).clamp(9.0, 40.0)
}

// ---------- 类型专属模板：按判型动态生成 sheet（列集各取所需） ----------
fn ident() -> Vec<(&'static str, &'static str)> {
    vec![("序号", "idx"), ("文件", "log_file"), ("相对路径", "rel_path"),
         ("工位", "station"), ("判型", "detected_type"), ("SN", "sn")]
}
fn device() -> Vec<(&'static str, &'static str)> {
    vec![("生产编号", "production_num"), ("系统SN", "system_sn"), ("板卡SN", "board_sn"),
         ("UUID", "uuid"), ("BIOS版本", "bios_version"), ("OS激活码", "os_key"),
         ("有线MAC", "lan"), ("无线MAC", "wifilan"), ("蓝牙MAC", "bluetooth")]
}
fn oa3_full() -> Vec<(&'static str, &'static str)> {
    vec![("OA3结果", "oa3_result"), ("ProductKeyID", "product_key_id"), ("PKState", "product_key_state"),
         ("ProductKey", "product_key"), ("Hash长度", "hardware_hash_len"),
         ("Hash SHA-256", "hardware_hash_sha256"), ("HardwareHash", "hardware_hash"),
         ("注入开始", "inject_start_at"), ("注入结束", "inject_end_at"), ("Baseboard", "baseboard_product")]
}
fn oa3_core() -> Vec<(&'static str, &'static str)> {
    vec![("ProductKeyID", "product_key_id"), ("PKState", "product_key_state"), ("ProductKey", "product_key")]
}
fn raw() -> Vec<(&'static str, &'static str)> {
    vec![("批次号", "mo_lot_no"), ("工位任务", "task_tag"), ("JSON状态", "json_state"),
         ("JSON res_value", "json_res_value"), ("项目版本", "project_version")]
}
fn result_cols() -> Vec<(&'static str, &'static str)> {
    vec![("提取状态", "extract_state"), ("回传状态", "upload_state"), ("退出码", "upload_code"),
         ("request_id", "request_id"), ("回传错误", "upload_error")]
}

/// 类型专属 sheet 模板（未来新判型：先落「明细」总表兜底，登记后即获得专属 sheet）
pub fn type_templates() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let join = |parts: Vec<Vec<(&'static str, &'static str)>>| -> Vec<(&'static str, &'static str)> {
        parts.into_iter().flatten().collect()
    };
    vec![
        ("etest(OA3)", join(vec![ident(), oa3_full(), raw(), result_cols()])),
        ("etest", join(vec![ident(), raw(), result_cols()])),
        ("e-autotest", join(vec![ident(), device(), raw(), result_cols()])),
        ("海格旧测试3", join(vec![ident(), device(), oa3_core(), raw(), result_cols()])),
        ("海格旧测试2", join(vec![ident(), device(), raw(), result_cols()])),
    ]
}

pub const AUDIT_COLS: [&str; 10] =
    ["时间", "SN", "日志文件", "回传状态", "退出码", "resp_status", "request_id", "尝试次数", "耗时s", "错误"];

/// 冻结窗格：cell 为右下侧起始单元格（openpyxl 语义，如 "C2" 冻结前 2 列 + 首行）
fn freeze_at(ws: &mut umya_spreadsheet::Worksheet, cell: &str, cols: f64, rows: f64) {
    let views = ws.get_sheet_views_mut();
    if views.get_sheet_view_list().is_empty() {
        views.add_sheet_view_list_mut(umya_spreadsheet::SheetView::default());
    }
    if let Some(sv) = views.get_sheet_view_list_mut().first_mut() {
        let mut pane = umya_spreadsheet::Pane::default();
        let mut top_left = umya_spreadsheet::Coordinate::default();
        top_left.set_coordinate(cell);
        pane.set_top_left_cell(top_left);
        pane.set_state(umya_spreadsheet::PaneStateValues::Frozen);
        pane.set_active_pane(if cols > 0.0 && rows > 0.0 {
            umya_spreadsheet::PaneValues::BottomRight
        } else if cols > 0.0 {
            umya_spreadsheet::PaneValues::TopRight
        } else {
            umya_spreadsheet::PaneValues::BottomLeft
        });
        if cols > 0.0 {
            pane.set_horizontal_split(cols);
        }
        if rows > 0.0 {
            pane.set_vertical_split(rows);
        }
        sv.set_pane(pane);
    }
}

/// 写一个 sheet：表头样式 + 批次自适应（空列隐藏/宽度自适应）+ 冻结 C2。
fn write_sheet(ws: &mut umya_spreadsheet::Worksheet, cols: &[(&str, &str)], rows: &[Row]) {
    for (i, (header, _)) in cols.iter().enumerate() {
        let cell = ws.get_cell_mut(((i + 1) as u32, 1u32));
        cell.set_value(*header);
        let mut styled = Font::default();
        styled.set_bold(true);
        let mut white = Color::default();
        white.set_argb("FFFFFFFF");
        styled.set_color(white);
        let mut align = Alignment::default();
        align.set_horizontal(HorizontalAlignmentValues::Center);
        align.set_vertical(VerticalAlignmentValues::Center);
        cell.get_style_mut()
            .set_font(styled)
            .set_alignment(align)
            .set_background_color_solid("FF4472C4");
    }
    for (ri, row) in rows.iter().enumerate() {
        let r = (ri + 2) as u32;
        ws.get_cell_mut((1u32, r)).set_value((ri + 1).to_string().as_str());
        for (ci, (_, key)) in cols.iter().enumerate().skip(1) {
            let v = clean_cell(&s(row, key));
            ws.get_cell_mut(((ci + 1) as u32, r)).set_value(v.as_str());
        }
    }
    for (j, (header, key)) in cols.iter().enumerate() {
        let letter = crate::core::exporter::column_letter(j + 1);
        let samples: Vec<String> = rows
            .iter()
            .map(|r| s(r, key))
            .filter(|v| !v.is_empty())
            .collect();
        if samples.is_empty() && *key != "idx" {
            ws.get_column_dimension_mut(&letter).set_hidden(true); // 本批未命中的字段整列隐藏
            continue;
        }
        let w = col_width(key, header, &samples);
        ws.get_column_dimension_mut(&letter).set_width(w);
        ws.get_column_dimension_mut(&letter).set_hidden(false);
    }
    freeze_at(ws, "C2", 2.0, 1.0); // 冻结表头 + 序号/文件两列
}

/// 明细 sheet + 摘要。返回实际生成文件路径。
pub fn export_filter_excel(path: &str, rows: &[Row], summary_text: &[(&str, String)]) -> anyhow::Result<String> {
    let mut book = umya_spreadsheet::new_file();
    {
        let ws = book.get_sheet_mut(&0).unwrap();
        ws.set_name("明细");
        let cols = detail_cols();
        write_sheet(ws, &cols, rows);
        // 类型专属 sheet：有该类型文件才生成
        for (type_name, cols) in type_templates() {
            let type_rows: Vec<Row> = rows
                .iter()
                .filter(|r| s(r, "detected_type") == type_name)
                .cloned()
                .collect();
            if type_rows.is_empty() {
                continue;
            }
            let name: String = type_name.chars().take(31).collect();
            let ws2 = book.new_sheet(&name).map_err(|e| anyhow::anyhow!("新建 sheet 失败: {e:?}"))?;
            write_sheet(ws2, &cols, &type_rows);
        }
        let ws3 = book.new_sheet("摘要").map_err(|e| anyhow::anyhow!("摘要 sheet: {e:?}"))?;
        ws3.get_cell_mut((1u32, 1u32)).set_value("项目");
        let mut bold = Font::default();
        bold.set_bold(true);
        ws3.get_cell_mut((1u32, 1u32)).get_style_mut().set_font(bold);
        ws3.get_cell_mut((2u32, 1u32)).set_value("内容");
        for (i, (k, v)) in summary_text.iter().enumerate() {
            let r = (i + 2) as u32;
            ws3.get_cell_mut((1u32, r)).set_value(*k);
            ws3.get_cell_mut((2u32, r)).set_value(v.as_str());
        }
        ws3.get_column_dimension_mut("A").set_width(18.0);
        ws3.get_column_dimension_mut("B").set_width(80.0);
    }
    umya_spreadsheet::writer::xlsx::write(&book, path).map_err(|e| anyhow::anyhow!("写 Excel 失败: {e:?}"))?;
    Ok(path.to_string())
}

/// 回传审计 CSV：sn/状态/退出码/request_id/耗时/错误。
pub fn write_upload_audit(path: &str, rows: &[Row]) -> anyhow::Result<String> {
    let mut out = String::from("\u{feff}");
    let push = |out: &mut String, cells: Vec<String>| {
        out.push_str(&cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    };
    push(&mut out, AUDIT_COLS.iter().map(|c| c.to_string()).collect());
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for r in rows {
        push(
            &mut out,
            vec![
                now.clone(),
                s(r, "sn"),
                s(r, "log_file"),
                s(r, "upload_state"),
                s(r, "upload_code"),
                s(r, "resp_status"),
                s(r, "request_id"),
                s(r, "upload_attempts"),
                s(r, "upload_elapsed"),
                clean_cell(&s(r, "upload_error")),
            ],
        );
    }
    std::fs::write(path, out)?;
    Ok(path.to_string())
}

fn csv_cell(c: &str) -> String {
    if c.contains(',') || c.contains('"') || c.contains('\n') || c.contains('\r') {
        format!("\"{}\"", c.replace('"', "\"\""))
    } else {
        c.to_string()
    }
}

/// 提取成功的日志按「目录/文件名」留存到批次目录，同名自动加序号。
pub fn copy_logs(items: &[Row], batch_dir: &str) -> usize {
    let mut copied = 0usize;
    for it in items {
        if it.get("extract_ok").and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let dir_name = {
            let d = s(it, "dir_name");
            if d.is_empty() { ".".to_string() } else { d }
        };
        let target_dir = Path::new(batch_dir).join(dir_name.replace('/', std::path::MAIN_SEPARATOR_STR));
        if std::fs::create_dir_all(&target_dir).is_err() {
            continue;
        }
        let filename = s(it, "filename");
        let mut dst = target_dir.join(&filename);
        if dst.exists() {
            let stem = Path::new(&filename).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            let ext = Path::new(&filename).extension().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
            let mut n = 2;
            while dst.exists() {
                let name = if ext.is_empty() { format!("{stem}_{n}") } else { format!("{stem}_{n}.{ext}") };
                dst = target_dir.join(name);
                n += 1;
            }
        }
        if std::fs::copy(s(it, "abs_path"), &dst).is_ok() {
            copied += 1;
        }
    }
    copied
}
