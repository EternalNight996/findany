//! 扫描核心：递归遍历、并发读文本、包含/不含判定、行号统计（对齐 sonar/scanner.py）。
//! 不依赖 UI：结果**按间隔分批**回传，界面边扫边出表；实时计数放在共享句柄里，
//! 界面每帧都能读到（不必等批事件），进度条与统计始终跟手。

use crate::core::config::SearchConfig;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// 单个文件的扫描结果。
#[derive(Debug, Clone, Default)]
pub struct ScanItem {
    pub dir_name: String,
    pub filename: String,
    pub rel_path: String,
    pub abs_path: String,
    pub hit: bool,
    pub hit_lines: Vec<usize>,
    pub hit_line_text: String,
    pub hit_count: usize,
    pub size: u64,
    pub mtime: f64,
    pub encoding: String,
    /// 若跳过，原因（too_large / binary / io_error / read_error）
    pub skipped: String,
}

impl ScanItem {
    pub fn mtime_str(&self) -> String {
        let secs = self.mtime as i64;
        match chrono::DateTime::from_timestamp(secs, 0) {
            Some(dt) => chrono::DateTime::<chrono::Local>::from(dt)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            None => String::new(),
        }
    }

    /// 与 Python `size_str` 一致：一位小数的 B/KB/MB/GB。
    pub fn size_str(&self) -> String {
        let mut v = self.size as f64;
        for unit in ["B", "KB", "MB", "GB"] {
            if v < 1024.0 || unit == "GB" {
                return format!("{v:.1} {unit}");
            }
            v /= 1024.0;
        }
        format!("{} B", self.size)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    pub total_files: usize,
    pub scanned: usize,
    pub hit: usize,
    pub miss: usize,
    pub skipped: usize,
    pub elapsed: f64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOutcome {
    pub items: Vec<ScanItem>,
    pub summary: ScanSummary,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LiveProgress {
    pub done: usize,
    pub total: usize,
    pub hit: usize,
    pub miss: usize,
    pub skipped: usize,
    pub pct: f64,
}

#[derive(Debug)]
pub enum ScanEvent {
    /// 进行中的一批结果（可能为空 items：仅更新计数，界面用于刷新数字）
    Batch(Box<ScanOutcome>),
    /// 收尾：完整结果 + 产物批次目录 + Excel 路径 + 复制份数
    /// （目录与文件分开带：界面「打开输出目录」必须拿到真实目录，不能拿拼接出来的展示字符串）
    Done(Box<ScanOutcome>, String, String, usize),
}

/// 扫描引擎：rayon 线程池 + 实时计数（界面共享同一份计数，读它就知道进度）
pub struct ScanEngine {
    pub cfg: SearchConfig,
    cancel: AtomicBool,
    hit_n: AtomicUsize,
    miss_n: AtomicUsize,
    skip_n: AtomicUsize,
    done_n: AtomicUsize,
    total_n: AtomicUsize,
}

impl ScanEngine {
    pub fn new(cfg: SearchConfig) -> Self {
        Self {
            cfg,
            cancel: AtomicBool::new(false),
            hit_n: AtomicUsize::new(0),
            miss_n: AtomicUsize::new(0),
            skip_n: AtomicUsize::new(0),
            done_n: AtomicUsize::new(0),
            total_n: AtomicUsize::new(0),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 已遍历到的待扫文件数（遍历阶段也能看到进度）
    pub fn last_total(&self) -> usize {
        self.total_n.load(Ordering::Relaxed)
    }

    pub fn last_hit(&self) -> usize {
        self.hit_n.load(Ordering::Relaxed)
    }

    /// 实时进度（界面每帧直接读，不必等批事件）
    pub fn progress(&self) -> LiveProgress {
        let total = self.total_n.load(Ordering::Relaxed);
        let done = self.done_n.load(Ordering::Relaxed);
        LiveProgress {
            done,
            total,
            hit: self.hit_n.load(Ordering::Relaxed),
            miss: self.miss_n.load(Ordering::Relaxed),
            skipped: self.skip_n.load(Ordering::Relaxed),
            pct: if total > 0 { done as f64 / total as f64 * 100.0 } else { 0.0 },
        }
    }

    /// 扫描：on_batch 按批间隔回调（返回 false 中止）
    pub fn scan(&self, mut on_batch: impl FnMut(ScanOutcome) -> bool) -> ScanOutcome {
        let cfg = &self.cfg;
        let t0 = std::time::Instant::now();
        let root = PathBuf::from(&cfg.root_dir);
        let mut files = if root.is_file() {
            vec![root.clone()]
        } else {
            walk_files(&root, &cfg.extensions, cfg.recursive)
        };
        // 遍历结果统一归一化：TEMP 可能给 8.3 短名（ADMINI~1），输出根/界面拿到的是长名，
        // 不归一会出现「同一目录两种写法」——输出目录排除与后续比对全会误判。
        let root_abs = canonical(&root);
        let root_plain = abs_path(&root);
        let root_is_file = root.is_file();
        for p in files.iter_mut() {
            if root_is_file {
                // 单文件模式：root 就是这个文件，没有「相对 root 的尾巴」。
                // 走下面那条路会得到 root_abs.join("") —— PathBuf::push 空串只在末尾补一个
                // 分隔符，而 canonical 出来是 \? 前缀路径，末尾带分隔符是非法路径：
                // metadata 直接失败 -> 一个结果都没有（界面表现：通用扫描选单文件没有任何结果）。
                *p = root_abs.clone();
                continue;
            }
            let tail = p.strip_prefix(&root).or_else(|_| p.strip_prefix(&root_plain)).unwrap_or(p.as_path());
            *p = root_abs.join(tail);
        }
        if !cfg.out_dir.is_empty() {
            let out_abs = canonical(Path::new(&cfg.out_dir));
            files.retain(|p| !under(p, &out_abs));
        }
        // 文件数上限（0=不限）：防目录跑飞；超了按上限收尾（界面/日志会看到总数被截）
        let cap = cfg.max_files.max(0) as usize;
        if cap > 0 && files.len() > cap {
            files.truncate(cap);
        }
        let total = files.len();
        self.total_n.store(total, Ordering::Relaxed);

        let mut all: Vec<ScanItem> = Vec::new();
        let mut pending: Vec<ScanItem> = Vec::new();
        let interval = std::time::Duration::from_millis(cfg.ui_refresh_ms.clamp(0, 5000) as u64);
        let throttle_ms = cfg.throttle_ms.clamp(0, 5000) as u64;
        let mut last_flush = std::time::Instant::now();
        const BATCH_MAX: usize = 64;

        let mut aborted = false;
        {
            // 关键：pending 里的结果必须**同时**留一份给总结论（all），再发一份给界面 ——
            // 界面拿到的是增量，结束时还要靠 all 落 Excel（漏了就会出现「有命中但产物为空」）
            let flush = |pending: &mut Vec<ScanItem>, all: &mut Vec<ScanItem>, on_batch: &mut dyn FnMut(ScanOutcome) -> bool| -> bool {
                if pending.is_empty() {
                    return true;
                }
                let batch = ScanOutcome { items: std::mem::take(pending), ..Default::default() };
                all.extend(batch.items.iter().cloned());
                if !on_batch(batch) {
                    return false;
                }
                // 资源节流：每批之间让一让（服务器上跑时给生产任务留 CPU/磁盘）
                let th = throttle_ms;
                if th > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(th));
                }
                true
            };
            let collect = |item: Option<ScanItem>| -> Option<ScanItem> {
                match item {
                    Some(it) if it.skipped.is_empty() => {
                        if it.hit {
                            self.hit_n.fetch_add(1, Ordering::Relaxed);
                        } else {
                            self.miss_n.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(it)
                    }
                    Some(_) => {
                        self.skip_n.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                    None => None,
                }
            };

            if cfg.threads <= 1 {
                for p in &files {
                    if self.cancelled() {
                        aborted = true;
                        break;
                    }
                    self.done_n.fetch_add(1, Ordering::Relaxed);
                    if let Some(it) = collect(match_one(p, cfg, &root_abs)) {
                        pending.push(it);
                    }
                    if pending.len() >= BATCH_MAX || (!interval.is_zero() && last_flush.elapsed() >= interval) {
                        last_flush = std::time::Instant::now();
                        if !flush(&mut pending, &mut all, &mut on_batch) {
                            aborted = true;
                            break;
                        }
                    }
                }
            } else {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(cfg.threads.clamp(1, 64) as usize).build().ok();
                let run = || {
                    files
                        .par_iter()
                        .map(|p| {
                            if self.cancelled() {
                                return None;
                            }
                            let r = match_one(p, cfg, &root_abs);
                            self.done_n.fetch_add(1, Ordering::Relaxed);
                            r
                        })
                        .collect::<Vec<_>>()
                };
                let results = match pool {
                    Some(p) => p.install(run),
                    None => run(),
                };
                for r in results {
                    if let Some(it) = collect(r) {
                        pending.push(it);
                    }
                    if pending.len() >= BATCH_MAX || (!interval.is_zero() && last_flush.elapsed() >= interval) {
                        last_flush = std::time::Instant::now();
                        if !flush(&mut pending, &mut all, &mut on_batch) {
                            aborted = true;
                            break;
                        }
                    }
                }
            }
            if !aborted {
                let _ = flush(&mut pending, &mut all, &mut on_batch);
            }
        }

        let skipped = self.skip_n.load(Ordering::Relaxed);
        let summary = ScanSummary {
            total_files: total,
            scanned: all.len(),
            hit: self.hit_n.load(Ordering::Relaxed),
            miss: self.miss_n.load(Ordering::Relaxed),
            skipped,
            elapsed: (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        };
        let _ = skipped;
        ScanOutcome { items: all, summary }
    }
}

/// 扫描完成后落产物：批次目录 + Excel（CSV 降级）+ 可选复制命中文件。
/// 返回（批次目录, xlsx 路径, 复制份数）
pub fn export_scan_products(items: &[ScanItem], summary: &ScanSummary, cfg: &SearchConfig) -> anyhow::Result<(String, String, usize)> {
    use crate::core::exporter;
    if items.is_empty() {
        // 没有任何文件参与：不产空 Excel，也不建空批次目录
        return Err(anyhow::anyhow!("无文件可导出"));
    }
    let batch = exporter::make_batch_dir(&cfg.out_dir)?;
    let xlsx = std::path::Path::new(&batch.path).join("scan_result.xlsx").to_string_lossy().to_string();
    let path = match exporter::export_excel(&xlsx, items, summary, cfg) {
        Ok(p) => p,
        Err(_) => exporter::export_csv(&xlsx, items, cfg)?,
    };
    let copied = if cfg.copy_files { exporter::copy_hits(items, &batch.path, &cfg.mode) } else { 0 };
    println!("[findany] 批次 {} 产出 {}", batch.name, path);
    Ok((batch.path, path, copied))
}

/// 引擎句柄：worker 线程与界面共享（取消 + 实时计数 + 遍历阶段进度）
pub type ScanHandle = Arc<ScanEngine>;

/// 在后台线程跑扫描：按间隔发批（界面表格按同样节奏实时增长）；结束发 Done。
pub fn spawn_scan(cfg: SearchConfig, tx: Sender<ScanEvent>) -> ScanHandle {
    let engine = Arc::new(ScanEngine::new(cfg.clone()));
    let handle = engine.clone();
    std::thread::spawn(move || {
        let e = engine.clone();
        let outcome = engine.scan(|batch| {
            tx.send(ScanEvent::Batch(Box::new(batch))).is_ok() && !e.cancelled()
        });
        let (dir, xlsx, copied) = match export_scan_products(&outcome.items, &outcome.summary, &cfg) {
            Ok(v) => v,
            Err(_) => (String::new(), String::new(), 0),
        };
        let _ = tx.send(ScanEvent::Done(Box::new(outcome), dir, xlsx, copied));
    });
    handle
}

// ---------- 编码读取 ----------

/// 编码探测：BOM → 零字节(UTF-16) → 启发式（对齐 Python chardet 的使用意图）。
pub fn detect_encoding(path: &Path) -> String {
    let raw = match read_head(path, 8192) {
        Some(b) => b,
        None => return "utf-8".into(),
    };
    if raw.is_empty() {
        return "utf-8".into();
    }
    detect_encoding_bytes(&raw)
}

pub fn detect_encoding_bytes(raw: &[u8]) -> String {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return "utf-8-sig".into();
    }
    if raw.starts_with(&[0xFF, 0xFE]) || raw.starts_with(&[0xFE, 0xFF]) {
        return "utf-16".into();
    }
    if raw.iter().take(4096).any(|b| *b == 0) {
        return "utf-16".into();
    }
    if raw.iter().any(|b| *b >= 0x80) {
        if std::str::from_utf8(raw).is_ok() {
            return "utf-8".into();
        }
        if encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(raw).is_some() {
            return "gbk".into();
        }
        return "latin-1".into();
    }
    "ascii".into()
}

fn read_head(path: &Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf).ok()?;
    buf.truncate(read);
    Some(buf)
}

/// 按编码候选链严格解码整个文件；全部失败返回 None（调用方记 read_error）。
pub fn decode_file(path: &Path, encoding: &str) -> Option<(String, String)> {
    let encodings: Vec<&str> = match encoding {
        "auto" => vec!["utf-8-sig", "utf-8", "gbk", "latin-1"],
        "utf-8" => vec!["utf-8-sig", "utf-8", "gbk"],
        "gbk" => vec!["gbk", "gb2312", "utf-8"],
        "utf-16" => vec!["utf-16", "utf-8-sig", "utf-8"],
        "ascii" => vec!["ascii", "latin-1", "utf-8"],
        other => vec![other],
    };
    let raw = std::fs::read(path).ok()?;
    for enc in encodings {
        if let Some(text) = strict_decode(&raw, enc) {
            return Some((text, enc.to_string()));
        }
    }
    None
}

/// 严格解码（不允许替换字符），对齐 Python errors="strict"。
pub fn strict_decode(raw: &[u8], enc: &str) -> Option<String> {
    let e = enc.to_ascii_lowercase();
    if e.starts_with("utf-16") {
        let endian = if e == "utf-16le" {
            encoding_rs::UTF_16LE
        } else if e == "utf-16be" {
            encoding_rs::UTF_16BE
        } else {
            encoding_rs::UTF_16LE
        };
        let (text, _) = endian.decode_with_bom_removal(raw);
        if text.contains('\u{FFFD}') {
            return None;
        }
        return Some(text.into_owned());
    }
    let codec = match e.as_str() {
        "utf-8" => encoding_rs::UTF_8,
        "utf-8-sig" => encoding_rs::UTF_8,
        "gbk" => encoding_rs::GBK,
        "gb2312" | "gb18030" => encoding_rs::GBK,
        "big5" => encoding_rs::BIG5,
        "latin-1" | "iso-8859-1" | "windows-1252" => encoding_rs::WINDOWS_1252,
        "ascii" | "us-ascii" => {
            return if raw.is_ascii() { Some(String::from_utf8_lossy(raw).into_owned()) } else { None }
        }
        _ => return None,
    };
    let text = codec.decode_without_bom_handling_and_without_replacement(raw)?;
    Some(text.into_owned())
}

/// Python 语义的行切分：保留行尾（\n / \r\n）。
fn lines_with_endings(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let b = text.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

// ---------- 文件遍历 ----------

fn ext_of(name: &str) -> Option<String> {
    match name.rfind('.') {
        Some(i) if i + 1 <= name.len() => Some(name[i + 1..].to_lowercase()),
        _ => None,
    }
}

fn ext_set(extensions: &[String]) -> std::collections::HashSet<String> {
    extensions
        .iter()
        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

fn matches_ext(name: &str, exts: &std::collections::HashSet<String>) -> bool {
    if exts.is_empty() {
        return true;
    }
    match ext_of(name) {
        Some(e) => exts.contains(&e),
        None => false,
    }
}

/// 收集待扫描文件路径（按扩展名过滤，空扩展名列表=全部；跳过 `.` 开头的目录与文件）。
pub fn walk_files(root: &Path, extensions: &[String], recursive: bool) -> Vec<PathBuf> {
    let exts = ext_set(extensions);
    let mut result = Vec::new();
    if recursive {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in &entries {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if matches_ext(&name, &exts) {
                    result.push(p);
                }
            }
        }
    } else if let Ok(rd) = std::fs::read_dir(root) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_file() && matches_ext(&name, &exts) {
                result.push(p);
            }
        }
    }
    result
}

pub fn abs_path(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

/// 路径归一化：绝对化 + 解析 8.3 短名（ADMINI~1 -> Administrator）。
/// Windows 的 canonicalize 会加 \`\\?\\` verbatim 前缀，与遍历出来的普通路径不相等 —— 这里剥掉。
pub fn canonical(p: &Path) -> PathBuf {
    let abs = abs_path(p);
    let c = std::fs::canonicalize(&abs).unwrap_or(abs);
    let s = c.to_string_lossy();
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        PathBuf::from(rest)
    } else {
        c
    }
}

/// 路径前缀比较：Windows 大小写不敏感，且必须落在目录边界上。
pub fn starts_with_ci(a: &Path, b: &Path) -> bool {
    let (a, b) = (a.to_string_lossy().to_lowercase(), b.to_string_lossy().to_lowercase());
    if !a.starts_with(&b) {
        return false;
    }
    match a.as_bytes().get(b.len()) {
        None => true,
        Some(c) => *c == b'/' || *c == b'\\',
    }
}

/// path 是否在 base 目录之下（语义对齐 Python os.path.commonpath，Windows 大小写不敏感）
pub fn under(path: &Path, base: &Path) -> bool {
    starts_with_ci(&canonical(path), &canonical(base))
}

// ---------- 单个文件匹配 ----------

/// 扫描单个文件；无法 stat 时返回 None。
pub fn match_one(path: &Path, cfg: &SearchConfig, root: &Path) -> Option<ScanItem> {
    let md = std::fs::metadata(path).ok()?;
    let size = md.len();
    let rel = pathdiff(path, root);
    let filename = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir_name = if rel.is_empty() || !rel.contains('/') {
        ".".to_string()
    } else {
        rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| ".".into())
    };
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    let mut item = ScanItem {
        dir_name,
        filename,
        rel_path: rel,
        abs_path: path.to_string_lossy().to_string(),
        size,
        mtime,
        ..Default::default()
    };

    if size as f64 > cfg.max_file_mb * 1024.0 * 1024.0 {
        item.skipped = "too_large".into();
        return Some(item);
    }
    let head = match read_head(path, 8192) {
        Some(h) => h,
        None => {
            item.skipped = "io_error".into();
            return Some(item);
        }
    };
    if !head.is_empty() && head.contains(&0u8) {
        let mut is_utf16 = matches!(cfg.encoding.as_str(), "utf-16" | "utf-16le" | "utf-16be");
        if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
            is_utf16 = true;
        }
        if !is_utf16 {
            item.skipped = "binary".into();
            return Some(item);
        }
        item.encoding = if cfg.encoding != "auto" { cfg.encoding.clone() } else { "utf-16".into() };
    } else {
        item.encoding = if cfg.encoding != "auto" { cfg.encoding.clone() } else { detect_encoding(path) };
    }

    let needle = if cfg.case_sensitive { cfg.keyword.clone() } else { cfg.keyword.to_lowercase() };

    let decoded = decode_file(path, &cfg.encoding);
    let (text, enc_used) = match decoded {
        Some(v) => v,
        None => {
            item.skipped = "read_error".into();
            return Some(item);
        }
    };

    let mut hit_lines: Vec<usize> = Vec::new();
    let mut hit_text = String::new();
    for (i, line) in lines_with_endings(&text).iter().enumerate() {
        let cmp = if cfg.case_sensitive { line.to_string() } else { line.to_lowercase() };
        if cmp.contains(&needle) {
            hit_lines.push(i + 1);
            if hit_text.is_empty() {
                hit_text = clean_cell(line.trim()).chars().take(200).collect();
            }
        }
    }

    item.encoding = enc_used;
    item.hit_count = hit_lines.len();
    item.hit_lines = hit_lines;
    item.hit_line_text = hit_text;
    let found_any = item.hit_count > 0;
    item.hit = if cfg.mode == "inc" { found_any } else { !found_any };
    Some(item)
}

/// 相对路径（统一 `/` 分隔，对齐 Python relpath + sep 替换）。
pub fn pathdiff(path: &Path, root: &Path) -> String {
    let p = abs_path(path);
    let r = abs_path(root);
    let rp = p.strip_prefix(&r).unwrap_or(&p).to_string_lossy().to_string();
    // 单文件模式：root 就是文件本身，strip 出来是空串 —— 用文件名顶上，
    // 否则表格「相对路径」列和 Excel 里都是空的（看着像没识别到）
    let rp = if rp.is_empty() {
        p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        rp
    };
    rp.replace('\\', "/")
}

/// 清理 Excel 非法控制字符（对齐 Python `_ILLEGAL`）。
pub fn clean_cell(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let v = *c as u32;
            !(v <= 0x08 || v == 0x0b || v == 0x0c || (0x0e..=0x1f).contains(&v) || v == 0x7f)
        })
        .collect()
}
