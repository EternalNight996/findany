//! 筛选引擎：目录遍历 -> 判型 -> 字段提取 -> （可选）逐台回传 -> 批次产物
//! （对齐 sonar/logfilter/engine.py）。不依赖 UI：进度/日志/结果**分批**经通道回传，
//! 界面边筛边出表，不必等全部跑完。

use super::extractors::{extract, read_text};
use super::report;
use super::types::{file_name, LogType};
use super::uploader::{resolve_cli, run_upload, UploadProfile, ST_CONFLICT, ST_DRY_RUN, ST_FAIL, ST_OK};
use crate::core::scanner::{pathdiff, walk_files_filtered};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
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
    /// 按一级子目录分批跑
    pub batch_dirs: bool,
    /// 分批时只挑名字含这段的子目录
    pub batch_name_filter: String,
    /// 每批之间的休眠（毫秒）：服务器上让路用
    pub throttle_ms: i64,
    /// 最多处理多少文件（0=不限）
    pub max_files: i64,
    /// 行缓存容量（与 SearchConfig.cache_capacity_rows 同源）：worker 端也走同一个阈值。
    /// 0=不限；>0 时超过立即淘汰最旧（FIFO），淘汰的暂留 pool 让 UI「加载更多」可拉回。
    pub cache_capacity_rows: i64,
    /// **处理策略**：`"scan"` = 通用扫描（按关键字匹配文件内容）；`"filter"` = 日志筛选 / 回传。
    /// 管道（遍历 / 并行 / 主表限容 / LRU 池 / 有界通道 / 看门狗 / 节流 / 取消 / 产物 / 表格渲染）
    /// 两者**完全共用**，唯一差异就是 `process_one` 按这个字段分派。
    pub mode: String,
    // ---- 下面是 scan 策略专用（mode="scan" 时才读）----
    pub keyword: String,
    /// inc 包含 | exc 不包含
    pub match_mode: String,
    pub case_sensitive: bool,
    // ---- 断点续扫 ----
    /// 进度文件（JSONL：一行一条已处理记录）。它同时就是**产物本体** ——
    /// 边跑边追加，中断后文件留着；跑完（未取消）就删掉（产物已有 Excel/CSV）。
    /// 空 = 不落进度（自检 / 无需续跑的场景可关）。
    pub progress_path: String,
    /// **已完成的文件夹**（绝对路径）：续跑时整个目录直接跳过 —— 这就是「缓存文件夹」的粒度。
    ///
    /// 为什么不用文件级跳过：一条日志一个文件时，进度表会涨到几十万行、续跑要把它全读进内存比对；
    /// 文件夹级只需几十~几百条，续跑判定是 O(目录数) 的集合查询，简单可靠。
    /// 代价：中断时正在跑的那个文件夹会整份重跑（服务端对重复上报返回 duplicate_accepted）。
    pub skip_dirs: std::collections::HashSet<String>,
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
            batch_dirs: false,
            batch_name_filter: String::new(),
            throttle_ms: 0,
            max_files: 0,
            cache_capacity_rows: 5000,
            mode: "filter".into(),
            keyword: "IT6563".into(),
            match_mode: "inc".into(),
            case_sensitive: false,
            progress_path: String::new(),
            skip_dirs: std::collections::HashSet::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilterSummary {
    pub total: usize,
    pub extracted: usize,
    pub unknown: usize,
    pub skipped: usize,
    /// 扫描模式专用：命中 / 未命中（筛选模式保持 0）
    pub hit: usize,
    pub miss: usize,
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
    /// 分批模式：总批次数 / 通过批次数 / 失败批次名
    pub batches_total: usize,
    pub batches_pass: usize,
    pub batch_fail_names: Vec<String>,
    /// 目录统计：有匹配文件的目录数 / 递归看到的子目录数（统计阶段产物，界面显示用）
    pub idx_dirs: usize,
    pub idx_sub_dirs: usize,
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
    /// 扫描模式：命中 / 未命中（筛选模式 0）
    pub hit: usize,
    pub miss: usize,
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

/// 路径规范化：Windows 路径大小写不敏感、正/反斜杠等价。
/// 目录级跳过（续跑）必须按规范化形式比较 —— 否则用户把 root_dir 写法换一下
/// （大小写 / 斜杠方向），已完成目录就全部对不上、整批从头重跑。
fn norm_key(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

/// 文件路径 → 所在目录（与 walk 给出的路径同一形式；文件夹级续跑按它判定）
fn parent_dir(p: &str) -> String {
    match p.rfind(['\\', '/']) {
        Some(i) if i > 0 => p[..i].to_string(),
        _ => String::new(),
    }
}

/// 落盘用的已完成文件夹列表：排序后写（顺序固定，人工看与比对都稳定）
fn sorted_dirs(done: &std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = done.iter().cloned().collect();
    v.sort();
    v
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

    // `log_type=auto` 时把 None 交给 extract（让它内部 detect）；
// 非 auto 时把强制类型交给 extract；**extract 内部会做 Bug-2A 真伪校验**（强制 OA3
// 但正文不含 OA3 锚点 → 降级到 detect）。这里不再 use_type 覆盖 detected_type，
// 而是让 extract 写权威字段，再读回来同步到 item.detected_type —— 防止 UI/Excel
// 显示"detected_type=etest(OA3)"但 Map 其它字段是 detect 路径的空值（看起来
// 像「OA3 提取成功」，实则假阳性）。
    let forced = if !cfg.log_type.is_empty() && cfg.log_type != LogType::Auto.as_str() {
        LogType::from_str(&cfg.log_type)
    } else {
        None
    };
    let fields = extract(path, &text, forced);
    let final_type = fields.get("detected_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    item.insert("detected_type".into(), Value::String(final_type));
    for (k, v) in fields.iter() {
        item.insert(k.clone(), v.clone());
    }
    item.insert("fields".into(), Value::Object(fields));
    item.insert("extract_ok".into(), Value::Bool(true));
    item
}

/// **统一管道的唯一策略点**：处理一个文件、产出一行。
///
/// - `mode="scan"`   → 通用扫描：按关键字匹配文件内容（含/不含），行里是路径与命中信息
/// - `mode="filter"` → 日志筛选回传：判型 + 字段提取，行里是 36 列业务字段
///
/// 其余（遍历、并行、限容、LRU 池、有界通道、看门狗、节流、取消、产物导出、表格渲染、
/// 按钮启停）两个模式**走的是同一份代码** —— 这是「两套实现各改各的、总有一边漏」的根治。
pub fn process_one(path: &str, root: &str, cfg: &FilterRunCfg) -> Map<String, Value> {
    if cfg.mode == "scan" {
        scan_one(path, root, cfg)
    } else {
        extract_one(path, root, cfg)
    }
}

/// 扫描策略：一个文件 → 一行（结构与筛选行同构，UI/导出/池都不必分支）。
fn scan_one(path: &str, root: &str, cfg: &FilterRunCfg) -> Map<String, Value> {
    let rel = pathdiff(Path::new(path), Path::new(root));
    let mut item = Map::new();
    item.insert("abs_path".into(), Value::String(path.to_string()));
    item.insert("rel_path".into(), Value::String(rel.clone()));
    item.insert("filename".into(), Value::String(file_name(path)));
    item.insert("dir_name".into(), Value::String(String::new()));
    item.insert("detected_type".into(), Value::String("scan".into()));
    item.insert("extract_ok".into(), Value::Bool(false));
    item.insert("error".into(), Value::String(String::new()));
    // 回传相关字段：扫描用不到，但保持行结构一致（导出模板/表格取值不必按模式分支）
    for k in ["upload_state", "upload_code", "request_id", "resp_status", "upload_error", "upload_attempts", "upload_elapsed"] {
        item.insert(k.into(), Value::String(String::new()));
    }

    let opts = crate::core::scanner::MatchOpts {
        keyword: &cfg.keyword,
        match_mode: &cfg.match_mode,
        case_sensitive: cfg.case_sensitive,
        encoding: &cfg.encoding,
        max_file_mb: cfg.max_file_mb,
    };
    let it = match crate::core::scanner::match_one_opts(Path::new(path), &opts, Path::new(root)) {
        Some(v) => v,
        None => {
            item.insert("error".into(), Value::String("无法读取文件属性".into()));
            return item;
        }
    };
    // 跳过（超大/二进制/读失败）：extract_ok 保持 false → 计入 skipped，并从表格里可见原因
    if !it.skipped.is_empty() {
        item.insert("error".into(), Value::String(it.skipped.clone()));
        item.insert("skip_reason".into(), Value::String(it.skipped.clone()));
        item.insert("size".into(), Value::Number(it.size.into()));
        item.insert("size_str".into(), Value::String(it.size_str()));
        item.insert("mtime_str".into(), Value::String(it.mtime_str()));
        return item;
    }

    item.insert("dir_name".into(), Value::String(it.dir_name.clone()));
    item.insert("hit".into(), Value::Bool(it.hit));
    item.insert("hit_state".into(), Value::String(if it.hit { "命中".into() } else { "未命中".into() }));
    item.insert(
        "hit_lines".into(),
        Value::String(it.hit_lines.iter().take(6).map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
    );
    item.insert("hit_line_text".into(), Value::String(it.hit_line_text.clone()));
    item.insert("hit_count".into(), Value::Number(it.hit_count.into()));
    item.insert("size".into(), Value::Number(it.size.into()));
    item.insert("size_str".into(), Value::String(it.size_str()));
    item.insert("mtime_str".into(), Value::String(it.mtime_str()));
    item.insert("encoding".into(), Value::String(it.encoding.clone()));
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
///
/// 注：行缓存池（row_pool）由 UI 端自己持有——worker 端不再跨线程共享一份 pool，
/// 简化了 LRU 淘汰与 emit 协议（不重复发同一行）。
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
    /// 扫描模式：命中 / 未命中
    pub hit_n: AtomicUsize,
    pub miss_n: AtomicUsize,
    /// 内存到上限被主动停（界面/结论用来写清原因）
    pub exceeded: AtomicBool,
    /// 统计阶段的结果：有匹配文件的目录数 / 递归看到的子目录数
    /// （界面「统计」那一栏用；统计完成标记由 treeindex 的 meta 持久化）
    pub idx_dirs: AtomicUsize,
    pub idx_sub_dirs: AtomicUsize,
    /// 统计是否已完整（true = 复用了上次的索引，没重新遍历）
    pub idx_ready: AtomicBool,
}

impl Default for FilterHandle {
    fn default() -> Self {
        Self {
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            extracted: AtomicUsize::new(0),
            unknown: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            upload_ok: AtomicUsize::new(0),
            upload_conflict: AtomicUsize::new(0),
            upload_fail: AtomicUsize::new(0),
            upload_dry: AtomicUsize::new(0),
            cancel: AtomicBool::new(false),
            hit_n: AtomicUsize::new(0),
            miss_n: AtomicUsize::new(0),
            exceeded: AtomicBool::new(false),
            idx_dirs: AtomicUsize::new(0),
            idx_sub_dirs: AtomicUsize::new(0),
            idx_ready: AtomicBool::new(false),
        }
    }
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
            hit: self.hit_n.load(Ordering::Relaxed),
            miss: self.miss_n.load(Ordering::Relaxed),
        }
    }
}

/// 跑一次筛选（阻塞）。events 为 None 时静默（测试用）。
pub fn run_filter(cfg: &FilterRunCfg, events: Option<&SyncSender<FilterEvent>>, cancel: &AtomicBool) -> (Vec<Map<String, Value>>, FilterSummary) {
    let handle = FilterHandle::default();
    run_filter_shared(cfg, events, cancel, &handle)
}

/// 单目录筛选（分批模式里每批走一遍这里）
pub fn run_filter_one(
    cfg: &FilterRunCfg,
    events: Option<&SyncSender<FilterEvent>>,
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
    // 计数基线：handle 的计数器在多批（batch_dirs）模式下是共享累加的，
    // 本批数量 = 结束时 - 开始时（不能直接读绝对值，否则第二批复述第一批）。
    // 也**不能**从主表 items 数（主表受 cache_capacity_rows 限制，会漏掉被淘汰的行）。
    let base_extracted = handle.extracted.load(Ordering::Relaxed);
    let base_unknown = handle.unknown.load(Ordering::Relaxed);
    let base_skipped = handle.skipped.load(Ordering::Relaxed);
    let base_hit = handle.hit_n.load(Ordering::Relaxed);
    let base_miss = handle.miss_n.load(Ordering::Relaxed);
    // ①-a **统计阶段**：先把目录树统计清楚并落盘（`_index_<指纹>.jsonl` + meta 的 completed 标记）。
    // 统计完整后**下次开始直接复用，不再重新遍历**；统计被中断也能接着数（已统计目录整棵跳过）。
    // 有了它，进度条分母与续跑弹窗里的总数才是真的（以前是边跑边累加，开始永远显示 0/0）。
    let idx = crate::core::logfilter::treeindex::ensure(cfg, cancel);
    let idx_total = idx.meta.total_files;
    handle.idx_dirs.store(idx.meta.total_dirs, Ordering::Relaxed);
    handle.idx_sub_dirs.store(idx.meta.sub_dirs, Ordering::Relaxed);
    handle.idx_ready.store(idx.meta.completed, Ordering::Relaxed);
    if idx_total > 0 {
        handle.total.store(idx_total, Ordering::Relaxed);
    }
    s.idx_dirs = idx.meta.total_dirs;
    s.idx_sub_dirs = idx.meta.sub_dirs;
    // 索引自检：meta 与目录清单对不上（写到一半被杀）就当没统计完，本次按遍历重扫
    if idx.meta.completed && idx.dirs.len() != idx.meta.total_dirs {
        log(
            "warn",
            &format!("目录统计与清单数量不一致（{} vs {}），本次按遍历重扫", idx.dirs.len(), idx.meta.total_dirs),
        );
    }
    log(
        "info",
        &format!(
            "目录统计{}：目录 {} 个（子目录 {} 个）/ 匹配文件 {} 个{}",
            if idx.reused { "（复用上次结果，不再重新统计）" } else if idx.meta.completed { "完成" } else { "（中断，下次接着数）" },
            idx.meta.total_dirs,
            idx.meta.sub_dirs,
            idx_total,
            if idx.meta.completed { " —— 已缓存，下次开始直接从这里往下跑" } else { "" }
        ),
    );
    // ①-b 不再预先遍历成清单（百万级目录会先吃 1GB 路径）：总数边遍历边累加
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
    send(FilterEvent::Progress { phase: "extract", done: 0, total: idx_total, pct: 0.0 });

    let extract_batch = |batch: &[String]| -> Vec<Map<String, Value>> {
        // 直接 collect<Vec>：rayon par_iter 已经产出 owned Map，无需再 cloned
        batch
            .par_iter()
            .map(|p| {
                let it = process_one(p, &cfg.root_dir, cfg);
                handle.done.fetch_add(1, Ordering::Relaxed);
                if it.get("extract_ok").and_then(|v| v.as_bool()) == Some(true) {
                    handle.extracted.fetch_add(1, Ordering::Relaxed);
                    if sval(it.get("detected_type")) == LogType::Unknown.as_str() {
                        handle.unknown.fetch_add(1, Ordering::Relaxed);
                    }
                    // 扫描模式：命中 / 未命中（同一管道里多记两个计数即可）
                    if cfg.mode == "scan" {
                        if it.get("hit").and_then(|v| v.as_bool()) == Some(true) {
                            handle.hit_n.fetch_add(1, Ordering::Relaxed);
                        } else {
                            handle.miss_n.fetch_add(1, Ordering::Relaxed);
                        }
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
    //
    // **内存上限（主表容量）**：`cache_capacity_rows`（默认 5000）。0=不限；>0 时超出立即
    // 淘汰最旧 —— 这是内存不失控的关键。此前这里用 `max_files`（默认 0=不限）导致 10 万行
    // × 真样本 13KB ≈ 1.3GB（实测 170MB→1400MB 的根因）。
    //
    // **只存一份 owned 数据**：主表就是唯一持有者，不再额外 clone 到"待发队列"（那会让内存翻倍）。
    // 用 `since_emit` 记录「自上次 emit 后新增了几行」，emit 时只取主表末尾这几行。
    // 旧实现用「单 Vec + emitted_offset 索引」，一旦淘汰就会打乱 offset 语义（已发过的行被移走
    // → 索引错位 → 同一行重复发或漏发）；末尾计数法在淘汰下天然正确。
    let worker_cap: usize = if cfg.cache_capacity_rows > 0 {
        cfg.cache_capacity_rows as usize
    } else if cfg.max_files > 0 {
        cfg.max_files as usize
    } else {
        usize::MAX
    };
    let pool = if cfg.threads <= 1 {
        None
    } else {
        rayon::ThreadPoolBuilder::new().num_threads(cfg.threads.clamp(1, 64) as usize).build().ok()
    };
    let mut items: std::collections::VecDeque<Map<String, Value>> = std::collections::VecDeque::new();
    // 自上次 emit 后新增的行数（emit 时只发主表末尾这几行，避免额外 clone 一份待发队列）
    let mut since_emit = 0usize;
    // 因容量上限被淘汰的行数（结论里要说清，避免用户以为「提取了却没进产物」是 bug）
    let mut evicted_rows: usize = 0;
    // 看门狗状态（每批一次采样）
    let mem = crate::core::mem_guard::new_guard();
    let mem_limit_mb = crate::core::mem_guard::effective_limit_mb(cfg.mem_limit_mb);
    let mut mem_check = std::time::Instant::now();
    let mut heartbeat = std::time::Instant::now();
    // ①-a 流式遍历：边遍历边处理，不再先把百万条路径全收进内存（那是大目录的第一块 GB 级分配）
    let mut discovered = 0usize;
    let mut walk_stopped = false;
    // 文件数上限（0=不限）：防目录跑飞的保险丝。**必须在这里截断** ——
    // 旧实现只在 walk_filter（预收集版）里截断，而统一管道走的是流式 walk_files_each，
    // 结果 max_files 对扫描/筛选都不生效（selftest 的 240->5 断言暴露）。
    let file_cap: usize = if cfg.max_files > 0 { cfg.max_files as usize } else { usize::MAX };
    // **进度节点**：开始时先写一份到 `<out_dir>/_resume.json` —— 它就是「上一次的任务记录」，
    // 重启/重开时 UI 读它决定要不要弹「继续上次」。每批刷新 done，跑完（导出成功）清掉。
    let node_out_dir = cfg.out_dir.clone();
    let node_on = !cfg.progress_path.is_empty() && !node_out_dir.trim().is_empty();
    let node_fp = if node_on { crate::core::logfilter::resume::task_fingerprint(cfg) } else { String::new() };
    let node_started = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if node_on {
        let n = crate::core::logfilter::resume::ResumeNode {
            version: 1,
            mode: cfg.mode.clone(),
            root_dir: cfg.root_dir.clone(),
            out_dir: node_out_dir.clone(),
            fingerprint: node_fp.clone(),
            total: idx_total, // 统计阶段给出的真实总数（复用上次索引时也是它）
            done: 0,
            started_at: node_started.clone(),
            updated_at: node_started.clone(),
            data_file: cfg.progress_path.clone(),
            done_files: Vec::new(),
            done_dirs: cfg.skip_dirs.iter().cloned().collect::<Vec<_>>(),
        };
        let _ = crate::core::logfilter::resume::save_node(&node_out_dir, &n);
    }
    // 每批刷新节点用的累计行数（与已落盘行数一致）
    let mut node_rows = 0usize;
    // ---- 文件夹级续跑的状态 ----
    // `done_dirs`：已跑完的文件夹（初始 = 上次留下的，续跑时逐个累加）；**一律存规范化路径**
    let mut done_dirs: std::collections::HashSet<String> =
        cfg.skip_dirs.iter().map(|d| norm_key(d)).collect();
    // 目录 → 是否在「已完成」集合之下（含自身）：遍历按目录推进，同一个目录只判一次
    let mut dir_skip_cache: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    // 上一个文件所在目录（规范化）：一旦出现新目录，说明上一个目录已经整个走完
    let mut last_dir: String = String::new();
    // 某个路径是否落在已完成的文件夹里（逐级向上找祖先，结果缓存）
    fn under_done(dir: &str, done: &std::collections::HashSet<String>, cache: &mut std::collections::HashMap<String, bool>) -> bool {
        if done.is_empty() {
            return false;
        }
        let dir = norm_key(dir);
        if let Some(v) = cache.get(&dir) {
            return *v;
        }
        let mut hit = false;
        let mut cur = dir.clone();
        loop {
            if done.contains(&cur) {
                hit = true;
                break;
            }
            match cur.rfind('\\') {
                // i <= 2 就到盘符/根了（C:\ 这种），不再往上
                Some(i) if i > 2 => {
                    cur.truncate(i);
                }
                _ => break,
            }
        }
        cache.insert(dir, hit);
        hit
    }
    let mut on_chunk = |chunk: &[String]| -> bool {
        // ① **断点续跑（文件夹级）**：已完成文件夹下的文件整批丢弃 —— 不重复提取/回传。
        //   注意顺序：先用「本批开始前」的已完成集合过滤，② 再把本批走完的目录记下来
        //   （只对后面的批生效）。反过来的话，本批第一个目录会被自己标记完成、把自己的文件全滤掉。
        let kept: Vec<String> = chunk
            .iter()
            .filter(|p| !under_done(&parent_dir(p), &done_dirs, &mut dir_skip_cache))
            .cloned()
            .collect();
        // ② 更新目录边界：出现新目录 = 上一个目录已经整个走完（walk 是深度优先、同目录连续）
        for p in chunk.iter() {
            let d = norm_key(&parent_dir(p));
            if !last_dir.is_empty() && last_dir != d {
                done_dirs.insert(last_dir.clone());
            }
            last_dir = d;
        }
        if kept.is_empty() {
            return true; // 整块都在已完成的文件夹里，继续下一块（不打扰取消/看门狗判断）
        }
        let chunk = kept.as_slice();
        // 到上限：本块按剩余额度截断，处理完就停（不再往下遍历）
        let remaining = file_cap.saturating_sub(discovered);
        if remaining == 0 {
            walk_stopped = true;
            return false;
        }
        let chunk = if chunk.len() > remaining { &chunk[..remaining] } else { chunk };
        discovered += chunk.len();
        let hit_cap = discovered >= file_cap;
        // 分母用统计阶段的真实总数（进度条才准）；没有索引时退回「边跑边累加」
        handle.total.store(if idx_total > 0 { idx_total } else { discovered }, Ordering::Relaxed);
        if cancel.load(Ordering::SeqCst) {
            walk_stopped = true;
            return false;
        }
        let run = || extract_batch(chunk);
        let got = match &pool {
            Some(p) => p.install(run),
            None => run(),
        };
        // 行逐条进主表：超 worker_cap 立即淘汰最旧（VecDeque::pop_front 是 O(1)，旧版 Vec::remove(0) 是 O(N)）。
        // **只存一份 owned 数据**：不再额外 clone 进待发队列（那会让内存翻倍）。
        // 用 `since_emit` 记录「自上次 emit 后新增了几行」，emit 时只取主表末尾这几行。
        for it in got {
            if items.len() >= worker_cap {
                items.pop_front();
                evicted_rows += 1;
            }
            items.push_back(it);
            since_emit += 1;
        }
        // 条数够了就发（chunk 本身就是一批），否则按间隔发 —— 小数据量也要分批，界面才看得见增长
        if since_emit >= BATCH || interval.is_zero() || last_emit.elapsed() >= interval {
            last_emit = std::time::Instant::now();
            // 取主表末尾 `since_emit` 行（O(since_emit)，不是 O(N)）：
            // VecDeque 是双端队列，rev().take(n).rev() 高效且保持原顺序。
            let tail: Vec<Map<String, Value>> =
                items.iter().rev().take(since_emit).rev().cloned().collect();
            // **增量落盘**（先写盘再推 UI，tail 随后被 move 走）：
            // 这个 JSONL 既是断点续扫的进度，也是中断时的产物本体。
            if !cfg.progress_path.is_empty() {
                if let Err(e) = append_progress(&cfg.progress_path, &tail) {
                    log("warn", &format!("进度落盘失败（中断后无法续跑）：{e}"));
                }
            }
            // 刷新进度节点（done = 已落盘行数、total = 本轮要处理的文件数）
            if node_on {
                node_rows += tail.len();
                let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let n = crate::core::logfilter::resume::ResumeNode {
                    version: 1,
                    mode: cfg.mode.clone(),
                    root_dir: cfg.root_dir.clone(),
                    out_dir: node_out_dir.clone(),
                    fingerprint: node_fp.clone(),
                    total: if idx_total > 0 { idx_total } else { discovered },
                    done: node_rows,
                    started_at: node_started.clone(),
                    updated_at: now,
                    data_file: cfg.progress_path.clone(),
                    done_files: Vec::new(),
                    done_dirs: sorted_dirs(&done_dirs),
                };
                let _ = crate::core::logfilter::resume::save_node(&node_out_dir, &n);
            }
            emit_batch(&send, tail, discovered, &handle);
            since_emit = 0;
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
        // 本块已把额度用满：收工（不再遍历后续文件）
        if hit_cap {
            walk_stopped = true;
            return false;
        }
        true
    };
    // **驱动方式**：索引已统计完整 → 按清单逐目录列文件（不再遍历全树，省一遍遍历、续跑起点精确到目录）；
    // 否则按目录树流式遍历。两者回调语义一致，处理逻辑是同一份。
    // 注意：root 本身是**单个文件**时不走清单（清单是「目录 → 文件数」，按它列会把这个目录下的
    // 兄弟文件全带上，而单文件模式只该处理那一个）。
    if idx.meta.completed && !idx.dirs.is_empty() && !Path::new(&cfg.root_dir).is_file() {
        crate::core::logfilter::treeindex::walk_dirs(&idx.dirs, &cfg.extensions, &cfg.name_filter, BATCH, &mut on_chunk);
    } else {
        crate::core::scanner::walk_files_each(
            Path::new(&cfg.root_dir),
            &cfg.extensions,
            cfg.recursive,
            &cfg.name_filter,
            BATCH,
            &mut on_chunk,
        );
    }
    let _ = walk_stopped;
    // 尾批：循环里可能因为没到间隔而没发出去，这里补发，保证界面拿到最后一批
    if since_emit > 0 {
        let tail: Vec<Map<String, Value>> =
            items.iter().rev().take(since_emit).rev().cloned().collect();
        if !cfg.progress_path.is_empty() {
            if let Err(e) = append_progress(&cfg.progress_path, &tail) {
                log("warn", &format!("进度落盘失败（中断后无法续跑）：{e}"));
            }
        }
        emit_batch(&send, tail, discovered, &handle);
    }
    s.total = discovered;

    // 提取阶段的计数：**从 handle 的差值**算（全量准确，不受主表容量淘汰影响）。
    // 旧实现遍历主表 items 计数 ── 主表被 cache_capacity_rows 限制后，10 万份只会报出 5000，
    // R 结论会误报「提取 5000」，看起来像丢数据。
    s.extracted = handle.extracted.load(Ordering::Relaxed).saturating_sub(base_extracted);
    s.unknown = handle.unknown.load(Ordering::Relaxed).saturating_sub(base_unknown);
    s.skipped = handle.skipped.load(Ordering::Relaxed).saturating_sub(base_skipped);
    s.hit = handle.hit_n.load(Ordering::Relaxed).saturating_sub(base_hit);
    s.miss = handle.miss_n.load(Ordering::Relaxed).saturating_sub(base_miss);
    // 容量淘汰提示：被淘汰的行不再参与产物/回传，必须在日志里说清
    // （否则用户看到「提取 10 万、产物只有 5000」会以为丢数据）
    if evicted_rows > 0 {
        log(
            "warn",
            &format!(
                "行缓存上限 {} 行：已淘汰最旧 {} 行不参与产物与回传（提取计数仍按全量 {} 统计）。调大「缓存行数」或勾「按子目录分批」可覆盖全量",
                worker_cap, evicted_rows, s.extracted
            ),
        );
    }
    // VecDeque -> Vec：一次 memcpy 级移动（5000 行 ≈ 10ms），换来主表容量可控。
    // 之后按 rel_path 排序供导出/回传（顺序稳定，UI 已在 Batch 流式接收时拿到自己的顺序）。
    let mut items: Vec<Map<String, Value>> = items.into();
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
                    if it.get("extract_ok").and_then(|v| v.as_bool()) != Some(true) {
                        return false;
                    }
                    // Bug-2B：回传 targets 必须**真含 OA3**（has_oa3=True）。
                    // Bug-2A 已经把「强制 OA3 但正文不含 OA3 锚点」的台 detected_type 改回 detect 真值，
                    // 这里再加 has_oa3 守卫：哪怕将来 detect 路径有漏网，targets 也不会把假 OA3 送进回传队列。
                    let dt = sval(it.get("detected_type"));
                    if dt == LogType::EtestOa3.as_str() && sval(it.get("has_oa3")) != "True" {
                        return false;
                    }
                    cfg.upload_types.contains(&dt)
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
                // cancel 透传到 default_runner：点停止时单次回传也能在 20ms 内中断子进程，
                // 否则 1万回传 + 默认 60s 超时 = 卡住几分钟。
                let res = run_upload(&profile, &fields, cfg.dry_run, &|lvl, msg| log(lvl, msg), Some(cancel));
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
                // 跑完了：进度文件的使命结束（产物已有 Excel / 审计 CSV），删掉它，
                // 否则下次点开始会被当成「未完成的任务」反复追问。
                if !cfg.progress_path.is_empty() {
                    let _ = std::fs::remove_file(&cfg.progress_path);
                }
                // 进度节点同理：跑完就清，别让下次点开始又问一遍
                if node_on {
                    crate::core::logfilter::resume::clear_node(&node_out_dir, &cfg.mode);
                }
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

/// 入口：`batch_dirs=true` 且 root 下超过一个一级子目录时**逐子目录跑**（内存只跟最大子目录有关）；
/// 关掉时直通单目录流程，行为与以前完全一致。每批各出产物、各判 PASS/FAIL，汇总计数给 R 结论。
pub fn run_filter_shared(
    cfg: &FilterRunCfg,
    events: Option<&SyncSender<FilterEvent>>,
    cancel: &AtomicBool,
    handle: &FilterHandle,
) -> (Vec<Map<String, Value>>, FilterSummary) {
    if cfg.batch_dirs {
        let mut subs: Vec<String> = std::fs::read_dir(&cfg.root_dir)
            .map(|rd| {
                let mut v: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| !n.starts_with('.'))
                    .filter(|n| {
                        let f = cfg.batch_name_filter.trim();
                        f.is_empty() || n.contains(f)
                    })
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        if subs.len() > 1 {
            let total_subs = subs.len();
            let mut agg = FilterSummary::default();
            let mut last_items: Vec<Map<String, Value>> = Vec::new();
            for (i, name) in subs.drain(..).enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let mut c = cfg.clone();
                c.root_dir = Path::new(&cfg.root_dir).join(&name).to_string_lossy().to_string();
                c.batch_dirs = false;
                if let Some(tx) = events {
                    let _ = tx.send(FilterEvent::Log(
                        "info".to_string(),
                        format!("批次 {}/{}：{}", i + 1, total_subs, c.root_dir),
                    ));
                }
                let (items_i, s_i) = run_filter_one(&c, events, cancel, handle);
                let ok = filter_verdict(&s_i, c.upload_enabled, c.dry_run).1;
                agg.batches_total += 1;
                if ok {
                    agg.batches_pass += 1;
                } else {
                    agg.batch_fail_names.push(name);
                }
                agg.total += s_i.total;
                agg.extracted += s_i.extracted;
                agg.unknown += s_i.unknown;
                agg.skipped += s_i.skipped;
                agg.upload_ok += s_i.upload_ok;
                agg.upload_conflict += s_i.upload_conflict;
                agg.upload_fail += s_i.upload_fail;
                agg.upload_dry += s_i.upload_dry;
                agg.upload_skip += s_i.upload_skip;
                agg.upload_targets += s_i.upload_targets;
                agg.elapsed += s_i.elapsed;
                if agg.batch_dir.is_empty() {
                    agg.batch_dir = s_i.batch_dir.clone();
                }
                if agg.mem_stop.is_empty() {
                    agg.mem_stop = s_i.mem_stop.clone();
                }
                // 只留最后一批给界面：内存有界正是这个模式的意义
                last_items = items_i;
            }
            return (last_items, agg);
        }
    }
    run_filter_one(cfg, events, cancel, handle)
}

/// Excel 降级阈值：超过这个行数只出 CSV（umya 生成 xlsx 时整本驻留内存）。
/// 从 50000 降到 10000：大目录（10万+）跑出来的 Excel 内存按行数放大，1万行
/// 已经是常见一台工位一整年的量，再多就是「没人会打开来看」的规模，强制走 CSV 更稳。
pub const EXCEL_MAX_ROWS: usize = 10_000;

/// 把一批行**追加**到进度文件（JSONL，一行一条）。
///
/// 这个文件既是「断点续扫的进度」也是「中断时的产物本体」：
/// - 每批 flush 一次（进程被杀也只丢最后未落盘的那一批）
/// - 一行一个 JSON 对象 → 恢复时能原样读回表格（主上要求「上次的行也载回表格」）
fn append_progress(path: &str, rows: &[Map<String, Value>]) -> std::io::Result<()> {
    use std::io::Write;
    if rows.is_empty() {
        return Ok(());
    }
    // 进度文件和产物同目录：目录可能还没建（首次跑）
    if let Some(dir) = Path::new(path).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut buf = String::new();
    for r in rows {
        match serde_json::to_string(r) {
            Ok(s) => {
                buf.push_str(&s);
                buf.push('\n');
            }
            Err(_) => continue, // 单行序列化失败不该拖垮整轮
        }
    }
    f.write_all(buf.as_bytes())?;
    f.flush()?; // 关键：中断时已落盘的不能丢
    Ok(())
}

/// 把一批结果推给界面：附实时计数，同批也会发进度事件
///
/// 接收 owned `Vec<Map>`，调用方 `items.drain(range).collect()` 出来直接发——
/// 避免旧版「再 to_vec 一份」造成的内存双拷贝。
/// **不再 emit 内排序**：UI 端按 push 顺序渲染（follow_tail 时自动滚到底），
/// 全表排序统一放到回传前的 `items.sort_by_key`（见 run_filter_one 末尾），
/// 减少 emit 路径上的 O(N) 开销。
fn emit_batch(send: &impl Fn(FilterEvent), items: Vec<Map<String, Value>>, total: usize, handle: &FilterHandle) {
    if items.is_empty() {
        return;
    }
    let live = handle.progress();
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
    // 不再 `items.to_vec()`：旧实现为了给每行补一个派生列 `extract_state` 而整体 clone 一份
    // （5000 行 × 13KB ≈ 65MB 峰值，十万行更甚）。该列现由 report::s() 当场从 extract_ok 派生。
    let rows: &[Map<String, Value>] = items;
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
///
/// `tx` 用 **SyncSender（有界通道）**：worker 推进速度远快于 UI 渲染时，无界通道会把
/// 全部结果堆在内存里（十万行 ≈ 2.6GB —— 实测峰值 600~800MB 的主因就在这里）。
/// 有界通道提供**背压**：队列满了 worker 的 send 就阻塞，等 UI 消费，内存因此封顶。
pub fn spawn_filter(cfg: FilterRunCfg, tx: SyncSender<FilterEvent>) -> Arc<FilterHandle> {
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
    // 分批模式：结论里必须写清批次（否则只看一行结论的人不知道跑了几批、哪批挂了）
    if summary.batches_total > 1 {
        let fails = if summary.batch_fail_names.is_empty() {
            String::new()
        } else {
            format!("，失败批次：{}", summary.batch_fail_names.join("/"))
        };
        return (
            format!(
                "分批 {} 批（PASS {} / FAIL {}），文件 {}，提取 {}（未知 {}，跳过 {}），回传 成功 {}/冲突 {}/失败 {}{}，耗时 {}s，产物 {}{}",
                summary.batches_total, summary.batches_pass, summary.batches_total - summary.batches_pass,
                summary.total, summary.extracted, summary.unknown, summary.skipped,
                summary.upload_ok, summary.upload_conflict, summary.upload_fail,
                if upload_enabled && dry_run { format!("（dry-run {}）", summary.upload_dry) } else { String::new() },
                summary.elapsed, summary.batch_dir, fails
            ),
            summary.batches_pass == summary.batches_total,
        );
    }
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
