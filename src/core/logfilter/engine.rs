//! 筛选引擎：目录遍历 -> 判型 -> 字段提取 -> （可选）逐台回传 -> 批次产物
//! （对齐 sonar/logfilter/engine.py）。不依赖 UI：进度/日志/结果**分批**经通道回传，
//! 界面边筛边出表，不必等全部跑完。

use super::extractors::{extract, read_text};
use super::report;
use super::types::{detect_log_type, file_name, LogType};
use super::uploader::{resolve_cli, run_upload, UploadProfile, ST_CONFLICT, ST_DRY_RUN, ST_FAIL, ST_OK};
use crate::core::scanner::{pathdiff, walk_files_filtered};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct FilterRunCfg {
    pub root_dir: String,
    /// 批次输出根
    pub out_dir: String,
    /// auto / etest(OA3) / etest / e-autotest / 海格旧测试2 / 海格旧测试3
    pub log_type: String,
    pub recursive: bool,
    pub extensions: Vec<String>,
    pub encoding: String,
    pub max_file_mb: f64,
    pub threads: i64,
    /// 提取成功日志留存
    pub keep_logs: bool,
    pub upload_enabled: bool,
    /// 参与回传的判型（定版②A：默认仅 OA3）
    pub upload_types: Vec<String>,
    pub dry_run: bool,
    pub profile: UploadProfile,
    /// 用于 CLI 默认路径解析；空=程序目录
    pub app_dir: String,
    /// 界面实时渲染间隔（毫秒）：每满这么久推一批给表格
    pub ui_refresh_ms: i64,
    /// 文件名包含（子串，不分大小写）：空=不过滤
    pub name_filter: String,
    /// 内存上限（MB）：超了主动安全停；0=物理内存 90%
    pub mem_limit_mb: i64,
    /// 每批之间的休眠（毫秒）：服务器上让路用
    pub throttle_ms: i64,
    /// 最多处理多少文件（0=不限）
    pub max_files: i64,
}

impl Default for FilterRunCfg {
    fn default() -> Self {
        Self {
            root_dir: String::new(),
            out_dir: String::new(),
            log_type: LogType::Auto.as_str().into(),
            recursive: true,
            extensions: vec!["log".into()],
            encoding: "auto".into(),
            max_file_mb: 20.0,
            threads: 8,
            keep_logs: true,
            upload_enabled: false,
            upload_types: vec![LogType::EtestOa3.as_str().into()],
            dry_run: true,
            profile: UploadProfile::default(),
            app_dir: String::new(),
            ui_refresh_ms: 200,
            name_filter: String::new(),
            mem_limit_mb: 0,
            throttle_ms: 0,
            max_files: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilterSummary {
    pub total: usize,
    pub extracted: usize,
    pub unknown: usize,
    pub skipped: usize,
    pub upload_ok: usize,
    pub upload_conflict: usize,
    pub upload_fail: usize,
    pub upload_dry: usize,
    pub upload_skip: usize,
    /// 定向类型（filter.types）匹配到的候选台数：为 0 说明"定向一台都没找到"
    pub upload_targets: usize,
    /// 没回传的原因（例如回传 CLI 未配置），用于拦截消息说清为什么
    pub upload_note: String,
    /// 内存到上限被主动安全停止的原因（空=没停）；会写进 R 结论
    pub mem_stop: String,
    pub elapsed: f64,
    pub batch_dir: String,
    pub excel_path: String,
    pub audit_path: String,
}

/// 实时计数（界面可直接读，不受分批节流影响）
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveFilterProgress {
    pub done: usize,
    pub total: usize,
    pub pct: f64,
    pub extracted: usize,
    pub unknown: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
pub struct FilterOutcome {
    pub items: Vec<Map<String, Value>>,
    pub summary: FilterSummary,
}

#[derive(Debug)]
pub enum FilterEvent {
    Log(String, String),
    /// 兼容事件：实时计数现在由 FilterHandle 直接读（不受节流影响），界面不再依赖本事件
    #[allow(dead_code)]
    Progress { phase: &'static str, done: usize, total: usize, pct: f64 },
    /// 进行中的一批提取结果（界面收到即 append -> 表格实时增长）
    Batch(Box<FilterOutcome>),
    /// 单行状态更新（回传中同一份日志的状态就地刷新：表格该行只改状态，不新增行）
    Row(Box<Map<String, Value>>),
    Upload { index: usize, total: usize, sn: String, status: String, pct: f64 },
    Done(Box<FilterOutcome>, String),
}

fn sval(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 提取单个文件（对齐 Python _extract_one）。
pub fn extract_one(path: &str, root: &str, cfg: &FilterRunCfg) -> Map<String, Value> {
    let rel = pathdiff(Path::new(path), Path::new(root));
    let mut item = Map::new();
    let dir_name = match rel.rsplit_once('/') {
        Some((d, _)) => d.to_string(),
        None => ".".to_string(),
    };
    item.insert("abs_path".into(), Value::String(path.to_string()));
    item.insert("rel_path".into(), Value::String(rel.clone()));
    item.insert("dir_name".into(), Value::String(dir_name));
    item.insert("filename".into(), Value::String(file_name(path)));
    item.insert("detected_type".into(), Value::String(String::new()));
    item.insert("extract_ok".into(), Value::Bool(false));
    item.insert("error".into(), Value::String(String::new()));
    for k in ["upload_state", "upload_code", "request_id", "resp_status", "upload_error", "upload_attempts", "upload_elapsed"] {
        item.insert(k.into(), Value::String(String::new()));
    }

    let md = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            item.insert("error".into(), Value::String(format!("读取失败: {e}")));
            return item;
        }
    };
    item.insert("size".into(), Value::Number(md.len().into()));
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    item.insert("mtime_str".into(), Value::String(mtime_str(mtime)));

    let text = match read_text(path, cfg.max_file_mb, &cfg.encoding) {
        Ok(t) => t,
        Err(e) => {
            item.insert("error".into(), Value::String(if e == "too_large" { "超大/超限跳过".into() } else { e }));
            return item;
        }
    };

    let forced = if !cfg.log_type.is_empty() && cfg.log_type != LogType::Auto.as_str() {
        LogType::from_str(&cfg.log_type)
    } else {
        None
    };
    let detected = detect_log_type(path, &text);
    let use_type = forced.unwrap_or(detected);
    item.insert("detected_type".into(), Value::String(use_type.as_str().into()));

    let fields = extract(path, &text, Some(use_type));
    for (k, v) in fields.iter() {
        item.insert(k.clone(), v.clone());
    }
    item.insert("fields".into(), Value::Object(fields));
    item.insert("extract_ok".into(), Value::Bool(true));
    item
}

fn mtime_str(secs: i64) -> String {
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => chrono::DateTime::<chrono::Local>::from(dt).format("%Y-%m-%d %H:%M:%S").to_string(),
        None => String::new(),
    }
}

fn under(path: &str, base: &str) -> bool {
    crate::core::scanner::under(Path::new(path), Path::new(base))
}

/// 目录遍历（扩展名过滤 + 输出目录排除 + 排序）。
/// root_dir 指向**单个文件**时就直接处理这一个文件（不再看扩展名——用户已经点名了它）。
/// 以前这里无脑走 walk_files：对文件 read_dir 会失败返回空，界面表现就是「选了单个文件识别不到」。
pub fn walk_filter(cfg: &FilterRunCfg) -> Vec<String> {
    let root = Path::new(&cfg.root_dir);
    // 文件数上限（0=不限）：防目录跑飞；超了截断（run 里会记一条警告）
    let cap = cfg.max_files.max(0) as usize;
    let mut out: Vec<String> = if root.is_file() {
        vec![root.to_string_lossy().to_string()]
    } else {
        walk_files_filtered(root, &cfg.extensions, cfg.recursive, &cfg.name_filter)
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    };
    if !cfg.out_dir.is_empty() {
        out.retain(|p| !under(p, &cfg.out_dir));
    }
    out.sort();
    if cap > 0 && out.len() > cap {
        out.truncate(cap);
    }
    out
}

/// 筛选句柄：worker 与界面共享（实时计数 + 取消）
#[derive(Default)]
pub struct FilterHandle {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub extracted: AtomicUsize,
    pub unknown: AtomicUsize,
    pub skipped: AtomicUsize,
    pub upload_ok: AtomicUsize,
    pub upload_conflict: AtomicUsize,
    pub upload_fail: AtomicUsize,
    pub upload_dry: AtomicUsize,
    pub cancel: AtomicBool,
    /// 内存到上限被主动停（界面/结论用来写清原因）
    pub exceeded: AtomicBool,
}

impl FilterHandle {
    pub fn progress(&self) -> LiveFilterProgress {
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        LiveFilterProgress {
            done,
            total,
            pct: if total > 0 { done as f64 / total as f64 * 100.0 } else { 0.0 },
            extracted: self.extracted.load(Ordering::Relaxed),
            unknown: self.unknown.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
        }
    }
}

/// 跑一次筛选（阻塞）。events 为 None 时静默（测试用）。
pub fn run_filter(cfg: &FilterRunCfg, events: Option<&Sender<FilterEvent>>, cancel: &AtomicBool) -> (Vec<Map<String, Value>>, FilterSummary) {
    let handle = FilterHandle::default();
    run_filter_shared(cfg, events, cancel, &handle)
}

/// 带共享句柄的筛选（GUI 用它读实时计数）
pub fn run_filter_shared(
    cfg: &FilterRunCfg,
    events: Option<&Sender<FilterEvent>>,
    cancel: &AtomicBool,
    handle: &FilterHandle,
) -> (Vec<Map<String, Value>>, FilterSummary) {
    let send = |ev: FilterEvent| {
        if let Some(tx) = events {
            let _ = tx.send(ev);
        }
    };
    let log = |lvl: &str, msg: &str| send(FilterEvent::Log(lvl.to_string(), msg.to_string()));

    let t0 = std::time::Instant::now();
    let mut s = FilterSummary::default();
    // ①-a 不再预先遍历成清单（百万级目录会先吃 1GB 路径）：总数边遍历边累加
    log(
        "info",
        &format!(
            "日志筛选：{} 类型={}，回传={}{}（遍历中，边遍历边处理）",
            cfg.root_dir,
            if cfg.log_type.is_empty() { "auto" } else { &cfg.log_type },
            if cfg.upload_enabled { "开" } else { "关" },
            if cfg.upload_enabled && cfg.dry_run { "（dry-run）" } else { "" }
        ),
    );
    send(FilterEvent::Progress { phase: "extract", done: 0, total: 0, pct: 0.0 });

    let extract_batch = |batch: &[String]| -> Vec<Map<String, Value>> {
        batch
            .par_iter()
            .map(|p| {
                let it = extract_one(p, &cfg.root_dir, cfg);
                handle.done.fetch_add(1, Ordering::Relaxed);
                if it.get("extract_ok").and_then(|v| v.as_bool()) == Some(true) {
                    handle.extracted.fetch_add(1, Ordering::Relaxed);
                    if sval(it.get("detected_type")) == LogType::Unknown.as_str() {
                        handle.unknown.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    handle.skipped.fetch_add(1, Ordering::Relaxed);
                }
                it
            })
            .collect()
    };

    // 分块 + 间隔双控：块号到了就停，但「够间隔」才推给界面 —— 渲染节奏可控（ui_refresh_ms）
    const BATCH: usize = 64;
    let interval = std::time::Duration::from_millis(cfg.ui_refresh_ms.clamp(0, 5000) as u64);
    let throttle_ms = cfg.throttle_ms.clamp(0, 5000) as u64;
    let mut last_emit = std::time::Instant::now();
    // ①-a：不再按文件总数预留（总数未知）；①-b 会把行改成边跑边落 CSV
    let mut items: Vec<Map<String, Value>> = Vec::new();
    let pool = if cfg.threads <= 1 {
        None
    } else {
        rayon::ThreadPoolBuilder::new().num_threads(cfg.threads.clamp(1, 64) as usize).build().ok()
    };
    let mut emitted = 0usize;
    // 看门狗状态（每批一次采样）
    let mem = crate::core::mem_guard::new_guard();
    let mem_limit_mb = crate::core::mem_guard::effective_limit_mb(cfg.mem_limit_mb);
    let mut mem_check = std::time::Instant::now();
    let mut heartbeat = std::time::Instant::now();
    // ①-a 流式遍历：边遍历边处理，不再先把百万条路径全收进内存（那是大目录的第一块 GB 级分配）
    let mut discovered = 0usize;
    let mut walk_stopped = false;
    crate::core::scanner::walk_files_each(Path::new(&cfg.root_dir), &cfg.extensions, cfg.recursive, &cfg.name_filter, BATCH, |chunk| {
        discovered += chunk.len();
        handle.total.store(discovered, Ordering::Relaxed);
        if cancel.load(Ordering::SeqCst) {
            walk_stopped = true;
            return false;
        }
        let run = || extract_batch(chunk);
        let got = match &pool {
            Some(p) => p.install(run),
            None => run(),
        };
        items.extend(got.iter().cloned());
        // 条数够了就发（chunk 本身就是一批），否则按间隔发 —— 小数据量也要分批，界面才看得见增长
        if got.len() >= BATCH || interval.is_zero() || last_emit.elapsed() >= interval {
            last_emit = std::time::Instant::now();
            emit_batch(&send, &got, discovered, &handle);
            emitted = items.len();
        }
        // 内存看门狗：到上限主动安全停止（百万级目录被系统挤爆时，进程内既没 panic 也没弹窗）
        {
            let done_now = handle.done.load(Ordering::Relaxed);
            if !crate::core::mem_guard::tick(&mem, mem_limit_mb, &mut mem_check, "筛选", done_now, discovered, &mut heartbeat) {
                handle.exceeded.store(true, Ordering::Relaxed);
                cancel.store(true, Ordering::SeqCst);
                walk_stopped = true;
                return false;
            }
        }
        // 资源节流：每批之间让一让（服务器上跑时给生产任务留 CPU/磁盘）
        if throttle_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(throttle_ms));
        }
        true
    });
    let _ = walk_stopped;
    // 尾批：循环里可能因为没到间隔而没发出去，这里补发，保证界面拿到全部行
    if emitted < items.len() {
        let tail: Vec<Map<String, Value>> = items[emitted..].to_vec();
        emit_batch(&send, &tail, discovered, &handle);
    }
    s.total = discovered;

    for it in &items {
        if it.get("extract_ok").and_then(|v| v.as_bool()) == Some(true) {
            s.extracted += 1;
            if sval(it.get("detected_type")) == LogType::Unknown.as_str() {
                s.unknown += 1;
            }
        } else {
            s.skipped += 1;
            log("warn", &format!("{}：{}", sval(it.get("rel_path")), sval(it.get("error"))));
        }
    }
    items.sort_by_key(|d| sval(d.get("rel_path")));

    // ---------- 回传（SOP：逐台串行，一次一台） ----------
    if cfg.upload_enabled && !cancel.load(Ordering::SeqCst) {
        let cli = resolve_cli(&cfg.profile.cli_path, &cfg.app_dir);
        let mut profile = cfg.profile.clone();
        profile.cli_path = cli.clone();
        if cli.is_empty() {
            s.upload_note = "回传 CLI 未配置（intunehelper_cli.exe）".into();
            log("err", "回传已启用但找不到 CLI（intunehelper_cli.exe），请在配置中选择");
        } else {
            log(
                "info",
                &format!(
                    "回传 CLI：{}{}  回传类型：{}",
                    cli,
                    if cfg.dry_run { "  [dry-run]" } else { "" },
                    cfg.upload_types.join(",")
                ),
            );
            let targets: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, it)| {
                    it.get("extract_ok").and_then(|v| v.as_bool()) == Some(true) && cfg.upload_types.contains(&sval(it.get("detected_type")))
                })
                .map(|(i, _)| i)
                .collect();
            let total = targets.len();
            s.upload_targets = total;
            for (n, idx) in targets.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let fields = items[*idx].get("fields").and_then(|v| v.as_object()).cloned().unwrap_or_default();
                let res = run_upload(&profile, &fields, cfg.dry_run, &|lvl, msg| log(lvl, msg));
                let rel = sval(items[*idx].get("rel_path"));
                {
                    let it = &mut items[*idx];
                    let state = match res.status.as_str() {
                        ST_OK => "成功",
                        ST_CONFLICT => "冲突(人工)",
                        ST_FAIL => "失败",
                        ST_DRY_RUN => "dry-run",
                        other => other,
                    };
                    it.insert("upload_state".into(), Value::String(state.into()));
                    it.insert("upload_code".into(), Value::String(res.exit_code.map(|c| c.to_string()).unwrap_or_default()));
                    it.insert("request_id".into(), Value::String(res.request_id.clone()));
                    it.insert("resp_status".into(), Value::String(res.resp_status.clone()));
                    it.insert("upload_error".into(), Value::String(res.error.clone()));
                    it.insert("upload_attempts".into(), Value::String(if res.attempts > 0 { res.attempts.to_string() } else { String::new() }));
                    it.insert("upload_elapsed".into(), Value::String(res.elapsed.to_string()));
                }
                match res.status.as_str() {
                    ST_OK => {
                        s.upload_ok += 1;
                        handle.upload_ok.fetch_add(1, Ordering::Relaxed);
                    }
                    ST_CONFLICT => {
                        s.upload_conflict += 1;
                        handle.upload_conflict.fetch_add(1, Ordering::Relaxed);
                    }
                    ST_FAIL => {
                        s.upload_fail += 1;
                        handle.upload_fail.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        s.upload_dry += 1;
                        handle.upload_dry.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if !res.error.is_empty() {
                    let lvl = if res.status == ST_CONFLICT || res.status == ST_DRY_RUN { "warn" } else { "err" };
                    log(lvl, &format!("[{}/{}] {} 回传：{}", n + 1, total, rel, res.error));
                } else {
                    log(
                        "ok",
                        &format!(
                            "[{}/{}] {} 回传：{}  request_id={}",
                            n + 1,
                            total,
                            rel,
                            if res.resp_status.is_empty() { "dry-run" } else { &res.resp_status },
                            if res.request_id.is_empty() { "-" } else { &res.request_id }
                        ),
                    );
                }
                send(FilterEvent::Upload {
                    index: n + 1,
                    total,
                    sn: sval(items[*idx].get("sn")),
                    status: res.status.clone(),
                    pct: if total > 0 { (n + 1) as f64 / total as f64 * 100.0 } else { 0.0 },
                });
                // 回传状态变化：只把这一行（已更新）推给界面就地刷新 —— 以前走 emit_batch 会被表格
                // 当成新行追加，同一份日志在表里出现多行，收尾时又被整表替换（看着就是「清空重建」）
                send(FilterEvent::Row(Box::new(items[*idx].clone())));
            }
            s.upload_skip = targets.iter().filter(|i| sval(items[**i].get("upload_state")).is_empty()).count();
        }
    } else if cfg.upload_enabled {
        log("warn", "已取消，跳过回传");
    }

    // ---------- 产物 ----------
    if !cancel.load(Ordering::SeqCst) && !items.is_empty() {
        match export_products(cfg, &items, &s, t0) {
            Ok((batch_dir, excel, audit, kept)) => {
                s.batch_dir = batch_dir.clone();
                s.excel_path = excel;
                s.audit_path = audit;
                log(
                    "ok",
                    &format!("产物：{batch_dir}  （Excel {} + 审计 upload-result.csv + 留存 {kept} 份日志）", file_name(&s.excel_path)),
                );
            }
            Err(e) => log("err", &format!("产物导出失败：{e}")),
        }
    }

    s.elapsed = ((t0.elapsed().as_secs_f64()) * 100.0).round() / 100.0;
    send(FilterEvent::Progress { phase: "done", done: s.total, total: s.total, pct: 100.0 });
    let tail = if s.upload_dry > 0 { format!(" / dry-run {}", s.upload_dry) } else { String::new() };
    log(
        "ok",
        &format!(
            "筛选完成：提取 {}/{}，未知 {}，跳过 {}，回传 成功 {} / 冲突 {} / 失败 {}{}，耗时 {}s",
            s.extracted, s.total, s.unknown, s.skipped, s.upload_ok, s.upload_conflict, s.upload_fail, tail, s.elapsed
        ),
    );
    (items, s)
}

/// Excel 降级阈值：超过这个行数只出 CSV（umya 生成 xlsx 时整本驻留内存）
pub const EXCEL_MAX_ROWS: usize = 50_000;

/// 把一批结果推给界面：附实时计数，同批也会发进度事件
fn emit_batch(send: &impl Fn(FilterEvent), batch: &[Map<String, Value>], total: usize, handle: &FilterHandle) {
    if batch.is_empty() {
        return;
    }
    let live = handle.progress();
    let mut items = batch.to_vec();
    items.sort_by_key(|d| sval(d.get("rel_path")));
    send(FilterEvent::Batch(Box::new(FilterOutcome {
        items,
        summary: FilterSummary {
            total,
            extracted: live.extracted,
            unknown: live.unknown,
            skipped: live.skipped,
            elapsed: 0.0,
            ..Default::default()
        },
    })));
    send(FilterEvent::Progress { phase: "extract", done: live.done, total, pct: live.pct });
}

/// 写产物（批次目录 + Excel + 审计 CSV + 可选留存日志）。
/// 界面「导出当前数据」直接复用同一条路径，保证手工导出与正常跑的产物结构一字不差。
pub fn export_products(
    cfg: &FilterRunCfg,
    items: &[Map<String, Value>],
    s: &FilterSummary,
    t0: std::time::Instant,
) -> anyhow::Result<(String, String, String, usize)> {
    let out_root = if cfg.out_dir.is_empty() {
        std::env::current_dir()?.to_string_lossy().to_string()
    } else {
        cfg.out_dir.clone()
    };
    let batch = crate::core::exporter::make_batch_dir(&out_root)?;
    let batch_dir = batch.path.clone();
    let mut rows: Vec<Map<String, Value>> = items.to_vec();
    for r in rows.iter_mut() {
        let state = if r.get("extract_ok").and_then(|v| v.as_bool()) == Some(true) { "成功" } else { "失败" };
        r.insert("extract_state".into(), Value::String(state.into()));
    }
    let summary_rows: Vec<(&str, String)> = vec![
        ("扫描目录", cfg.root_dir.clone()),
        ("筛选类型", if cfg.log_type.is_empty() { "auto".into() } else { cfg.log_type.clone() }),
        ("文件总数", s.total.to_string()),
        ("提取成功", s.extracted.to_string()),
        ("未知类型", s.unknown.to_string()),
        ("跳过", s.skipped.to_string()),
        (
            "回传开关",
            if cfg.upload_enabled {
                format!("开{}", if cfg.dry_run { "（dry-run）" } else { "" })
            } else {
                "关".to_string()
            },
        ),
        ("回传成功", s.upload_ok.to_string()),
        ("回传冲突", s.upload_conflict.to_string()),
        ("回传失败", s.upload_fail.to_string()),
        ("耗时(秒)", format!("{}", ((t0.elapsed().as_secs_f64()) * 100.0).round() / 100.0)),
        ("导出时间", chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
    ];
    // 行数超阈值：Excel 要在内存里拿全表（umya 整本都驻留），几万行以上就顶出几 GB —— 大任务只出 CSV，
    // 并把这件事写进 R 结论（看日志的人必须知道"为什么没有 xlsx"）
    let audit = report::write_upload_audit(&PathBuf::from(&batch_dir).join("upload-result.csv").to_string_lossy(), &rows)?;
    let excel = if rows.len() > EXCEL_MAX_ROWS {
        // 大任务降级：不生成 xlsx（umya 整本驻留内存，几万行以上会顶出几 GB），明细看审计 CSV；
        // 这里返回一句说明，会被打进运行日志与「产物」提示里
        format!("（行数 {} 超阈值 {}，已跳过 Excel，明细见 upload-result.csv）", rows.len(), EXCEL_MAX_ROWS)
    } else {
        report::export_filter_excel(&PathBuf::from(&batch_dir).join("filter_result.xlsx").to_string_lossy(), &rows, &summary_rows)?
    };
    let kept = if cfg.keep_logs { report::copy_logs(&rows, &batch_dir) } else { 0 };
    Ok((batch_dir, excel, audit, kept))
}

/// 后台线程跑筛选（边筛边分批回传），返回共享句柄（取消 + 实时计数）
pub fn spawn_filter(cfg: FilterRunCfg, tx: Sender<FilterEvent>) -> Arc<FilterHandle> {
    let handle = Arc::new(FilterHandle::default());
    let h = handle.clone();
    std::thread::spawn(move || {
        let mut cfg = cfg;
        if cfg.app_dir.is_empty() {
            cfg.app_dir = crate::core::app_dir::app_dir().to_string_lossy().to_string();
        }
        let (items, summary) = run_filter_shared(&cfg, Some(&tx), &h.cancel, &h);
        let _ = tx.send(FilterEvent::Done(Box::new(FilterOutcome { items, summary }), String::new()));
    });
    handle
}

/// 回传异常判定（None=正常）。开了正式回传就必须真的回传：
///  · 定向类型一台都没匹配到（只提取、没回传）
///  · 定向台数里有一台都没回传（被中断）
///  · 有失败/冲突
/// dry-run 演练与关闭回传不算异常。
pub fn upload_anomaly(summary: &FilterSummary, upload_enabled: bool, dry_run: bool) -> Option<String> {
    if !upload_enabled || dry_run {
        return None;
    }
    let ran = summary.upload_ok + summary.upload_conflict + summary.upload_fail;
    if ran == 0 && summary.upload_targets == 0 {
        let why = if summary.upload_note.is_empty() { "" } else { summary.upload_note.as_str() };
        return Some(if why.is_empty() {
            format!("回传已开启但一台都没回传：定向类型在这些日志里一台都没匹配到（提取 {} 台）", summary.extracted)
        } else {
            format!("回传已开启但一台都没回传：{why}")
        });
    }
    if ran == 0 {
        return Some(format!("定向 {} 台一台都没有回传（中断/未执行）", summary.upload_targets));
    }
    if summary.upload_skip > 0 {
        return Some(format!("定向 {} 台里有 {} 台没有回传（中断）", summary.upload_targets, summary.upload_skip));
    }
    if summary.upload_fail > 0 || summary.upload_conflict > 0 {
        return Some(format!("回传异常：成功 {} / 冲突 {} / 失败 {}", summary.upload_ok, summary.upload_conflict, summary.upload_fail));
    }
    None
}

/// 筛选结论：空数据 / 回传异常 → status=false（GUI 与 --auto 共用一套判定）
pub fn filter_verdict(summary: &FilterSummary, upload_enabled: bool, dry_run: bool) -> (String, bool) {
    // 内存到上限被主动停：一定要出现在结论里（否则看日志的人以为是程序崩了）
    if !summary.mem_stop.is_empty() {
        return (
            format!(
                "{}，提取 {}/{}，产物 {}",
                summary.mem_stop, summary.extracted, summary.total, summary.batch_dir
            ),
            false,
        );
    }
    if summary.total == 0 || summary.extracted == 0 {
        return (format!("数据为空：文件 {}，提取 {}", summary.total, summary.extracted), false);
    }
    if let Some(why) = upload_anomaly(summary, upload_enabled, dry_run) {
        return (
            format!(
                "{why}，提取 {}/{}，产物 {}",
                summary.extracted, summary.total, summary.batch_dir
            ),
            false,
        );
    }
    let up = if upload_enabled {
        if dry_run {
            format!("，回传 dry-run {}", summary.upload_dry)
        } else {
            format!("，回传 成功 {}/冲突 {}/失败 {}", summary.upload_ok, summary.upload_conflict, summary.upload_fail)
        }
    } else {
        String::new()
    };
    (
        format!(
            "提取 {}/{}（未知 {}，跳过 {}）{up}，耗时 {}s，产物 {}",
            summary.extracted, summary.total, summary.unknown, summary.skipped, summary.elapsed, summary.batch_dir
        ),
        true,
    )
}
