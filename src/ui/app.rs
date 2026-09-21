//! egui 主界面（1:1 对齐 Python 版 PySide6 布局与流程）：
//! 顶栏 / 左侧配置面板（440px，可收放）/ 右侧统计+结果表 / 底部日志条 / 完成倒计时。
//!
//! v2 约定：
//!   - 唯一配置文件 findany.toml；任何设置变动 500ms 后自动落盘（按钮另有即时反馈）
//!   - 整体字号走 theme::SIZE_*，不再硬编码
//!   - 已移除 SN 关联 / 单文件筛选方案（不需要）

use crate::core::config::SearchConfig;
use crate::core::logfilter::engine::{self as fengine, FilterEvent, FilterOutcome, FilterSummary};
use crate::core::logfilter::types::LogType;
use crate::core::scanner::{LiveProgress, ScanItem, ScanSummary};
use crate::ui::theme::{self, Palette, BTN_H, ROW_H, SIZE_BODY, SIZE_HEAD, SIZE_SMALL, SIZE_STAT, SIZE_TITLE};
use eframe::egui;
use egui::{Color32, Rangef, RichText};
use serde_json::{Map, Value};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PANEL_W: f32 = 440.0;
const PANEL_W_FOLDED: f32 = 76.0;
const FLASH_MS: u64 = 1200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkMode {
    Scan,
    Filter,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveState {
    Idle,
    Saved,
    Failed,
}

enum Running {
    Idle,
    Scan {
        rx: Receiver<crate::core::scanner::ScanEvent>,
        t0: Instant,
        handle: crate::core::scanner::ScanHandle,
    },
    Filter {
        /// 实时计数 + 取消都在句柄里（不受分批节流影响）
        handle: Arc<fengine::FilterHandle>,
        rx: Receiver<FilterEvent>,
        t0: Instant,
        up_index: usize,
        up_total: usize,
        up_pct: f64,
        sn: String,
        status: String,
    },
}

pub struct FindanyApp {
    cfg: SearchConfig,
    /// 上次落盘时的配置快照（用于顶栏「有改动，点保存配置」提示）
    snapshot: SearchConfig,
    mode: WorkMode,
    dark: bool,
    panel_open: bool,
    running: Running,
    items: Vec<ScanItem>,
    filter_items: Vec<Map<String, Value>>,
    scan_summary: ScanSummary,
    summary: FilterSummary,
    logs: Vec<(String, String, String)>,
    log_open: bool,
    countdown: Option<u32>,
    last_tick: Option<Instant>,
    auto_mode: bool,
    auto_close_window: bool,
    need_repaint: bool,
    save_state: SaveState,
    save_flash_at: Option<Instant>,
    /// 本帧事件未消费完（数据量大时正常出现）
    events_backlogged: bool,
    /// 日志去抖状态：(消息前缀, 最后时间, 累计条数)
    log_last: Option<(String, Instant, usize)>,
    /// 操作反馈（任何操作都必须有反馈：文字 + 级别）
    status: (String, &'static str),
    /// 「启动中」：点击开始后到首批结果到达之间的过渡态（按钮立刻变，不等 worker）
    starting: bool,
    /// 结果表抬底跟随最新一行（运行中开启，滚动条被用户拖动后关闭）
    follow_tail: bool,
    /// 上一帧的行数（用于判断"本帧有新内容"→ 抬底）
    last_row_count: usize,
    /// 本帧是否有新行
    rows_changed: bool,
    /// 上一次配置变更检测时间（dirty 检测本身要序列化配置，别每帧都做）
    last_dirty_check: Option<Instant>,
    /// 配置与已落盘内容是否不一致（顶栏提示用；节流每 250ms 更新一次）
    dirty_now: bool,
    /// 扫描目标识别结果缓存：(被检测的路径, 结果)。检测走后台线程，UI 线程只读它
    root_kind: std::sync::Arc<std::sync::Mutex<(String, PathKind)>>,
    /// 待落盘的界面日志（攒一批交给写线程：每行都开关文件会把 UI 线程拖卡）
    log_pending: Vec<(String, String)>,
    log_flush_at: Option<Instant>,
    /// 日志写线程的队列（UI 线程只 send，不碰文件）
    log_tx: std::sync::mpsc::Sender<Vec<(String, String)>>,
    /// 后台任务（导出 / 打开目录）结果：(给人看的消息, 产物目录)。UI 线程只 try_recv
    job_rx: Option<Receiver<(String, String)>>,
    scan_out_dir: String,
    font_note: String,
    proc_dir: std::path::PathBuf,
}

impl FindanyApp {
    pub fn new(cfg: SearchConfig, proc_dir: std::path::PathBuf, font_note: String, auto_mode: bool) -> Self {
        let start_mode = if cfg.work_mode == "filter" { WorkMode::Filter } else { WorkMode::Scan };
        let mut logs = Vec::new();
        logs.push((
            chrono::Local::now().format("%H:%M:%S").to_string(),
            "info".to_string(),
            format!("配置文件：{}", crate::core::logfilter::autoconfig::config_path().display()),
        ));
        let app = Self {
            snapshot: cfg.clone(),
            cfg,
            mode: start_mode,
            dark: true,
            panel_open: true,
            running: Running::Idle,
            items: Vec::new(),
            filter_items: Vec::new(),
            scan_summary: ScanSummary::default(),
            summary: FilterSummary::default(),
            logs,
            log_open: false,
            countdown: None,
            last_tick: None,
            auto_mode,
            auto_close_window: false,
            need_repaint: false,
            save_state: SaveState::Idle,
            save_flash_at: None,
            events_backlogged: false,
            log_last: None,
            status: ("就绪：选好目录后点「开始扫描」".to_string(), "info"),
            starting: false,
            follow_tail: true,
            last_row_count: 0,
            rows_changed: false,
            last_dirty_check: None,
            dirty_now: false,
            root_kind: std::sync::Arc::new(std::sync::Mutex::new((String::new(), PathKind::Missing))),
            log_pending: Vec::new(),
            log_flush_at: None,
            log_tx: make_log_writer(),
            job_rx: None,
            scan_out_dir: String::new(),
            font_note,
            proc_dir,
        };
        app
    }

    fn palette(&self) -> Palette {
        if self.dark { theme::dark() } else { theme::light() }
    }

    /// 统一反馈入口：状态条 + 日志，一次调用两处都有（不允许"点了没反应"）
    fn feedback(&mut self, lvl: &'static str, msg: impl Into<String>) {
        let msg = msg.into();
        self.status = (msg.clone(), lvl);
        self.push_log(lvl, &msg);
    }

    fn push_log(&mut self, lvl: &str, msg: &str) {
        // 日志去抖：数据量大时同一类消息（"跳过 xxx"）会刷出上万条，界面日志区与日志文件都受不了。
        // 同前缀消息 3 秒内的重复只累加计数，最后一行改成"…（同类 N 条）"。
        const LOG_BUDGET: usize = 600;
        let key: String = msg.chars().take(24).collect();
        let now = Instant::now();
        if let Some((last_key, at, n)) = self.log_last.as_mut() {
            if *last_key == key && at.elapsed() < Duration::from_secs(3) {
                *n += 1;
                *at = now;
                let dup = *n;
                if let Some(last) = self.logs.last_mut() {
                    last.2 = format!("{msg}　（同类 {dup} 条）");
                }
                self.log_pending.push((lvl.to_string(), msg.to_string()));
                return;
            }
        }
        self.log_last = Some((key, now, 1));
        let t = chrono::Local::now().format("%H:%M:%S").to_string();
        self.logs.push((t, lvl.to_string(), msg.to_string()));
        if self.logs.len() > LOG_BUDGET {
            // 一次砍掉四分之一：摊还 O(1)，避免每帧都 drain
            let cut = LOG_BUDGET / 4;
            self.logs.drain(0..cut);
        }
        // 文件落盘走批量缓冲（见 flush_logs）：每条都 open/append/close 会把 UI 线程拖卡
        self.log_pending.push((lvl.to_string(), msg.to_string()));
        if self.log_pending.len() >= 64 {
            self.flush_logs();
        }
    }

    /// 把缓冲的日志交给写线程（最多每 250ms 一次；超过 65 条立刻投，别让日志迟到）。
    /// UI 线程只做 send，不碰文件。
    fn flush_logs(&mut self) {
        if self.log_pending.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.log_pending);
        let _ = self.log_tx.send(batch);
        self.log_flush_at = Some(Instant::now());
    }

    /// 有任务在跑（含「已点击、worker 尚未出首批」的过渡态）
    fn running(&self) -> bool {
        !matches!(self.running, Running::Idle) || self.starting
    }

    /// 真正在跑（过渡态不算）——用于判断按钮该显示"启动中"还是"停止"
    fn busy(&self) -> bool {
        !matches!(self.running, Running::Idle)
    }

    /// 已请求停止（句柄取消标志已置位）——按钮切"正在停止…"，避免"点了没反应"
    fn cancelling(&self) -> bool {
        match &self.running {
            Running::Scan { handle, .. } => handle.cancelled(),
            Running::Filter { handle, .. } => handle.cancel.load(Ordering::Relaxed),
            Running::Idle => false,
        }
    }

    // ---------- 配置：单一文件 findany.toml ----------

    fn cfg_dirty(&self) -> bool {
        serde_json::to_value(&self.cfg).ok() != serde_json::to_value(&self.snapshot).ok()
    }


    /// 写回 findany.toml（**只有用户点「保存配置」会走到这里** —— 不再有自动保存）
    fn save_now(&mut self) -> bool {
        let path = crate::core::logfilter::autoconfig::config_path();
        match crate::core::logfilter::autoconfig::save_config(&path, &self.cfg) {
            Ok(p) => {
                self.snapshot = self.cfg.clone();
                self.dirty_now = false;
                self.push_log("ok", &format!("配置已保存：{p}"));
                true
            }
            Err(e) => {
                self.push_log("err", &format!("配置保存失败：{e}"));
                false
            }
        }
    }

    fn mark_saved(&mut self, ok: bool) {
        self.save_state = if ok { SaveState::Saved } else { SaveState::Failed };
        self.save_flash_at = Some(Instant::now());
    }

    /// 只做两件事：手动保存按钮的状态闪回 + 「配置是不是和文件不一致」的检测。
/// **不做自动落盘**：配置只在用户点「保存配置」时写回 findany.toml（用户明确要求）。
    fn tick_save(&mut self, ctx: &egui::Context) {
        // 手动保存按钮的反馈还原（1.2s）
        if let Some(t) = self.save_flash_at {
            if t.elapsed() >= Duration::from_millis(FLASH_MS) {
                self.save_state = SaveState::Idle;
                self.save_flash_at = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
        // 脏检测要序列化整份配置，别每帧做：250ms 一次足够（顶栏那个「有改动待保存」提示用）
        let due_check = match self.last_dirty_check {
            Some(t) => t.elapsed() >= Duration::from_millis(250),
            None => true,
        };
        if !due_check {
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }
        self.last_dirty_check = Some(Instant::now());
        self.dirty_now = self.cfg_dirty();
        // 日志批量落盘：最多 250ms 一次（启动/收尾时也要保证最终都写下去）
        let flush_due = match self.log_flush_at {
            Some(t) => t.elapsed() >= Duration::from_millis(250),
            None => true,
        };
        if flush_due && !self.log_pending.is_empty() {
            self.flush_logs();
        } else if !self.log_pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if self.dirty_now {
            // 有改动就别让界面睡太久：顶栏提示要实时跟着变
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    // ---------- 启动 / 停止 ----------

    fn start_scan(&mut self) {
        if self.running() || self.starting {
            self.feedback("warn", "已在运行中，忽略重复点击");
            return;
        }
        self.cfg.resolve_out_dir(&self.proc_dir);
        // 校验失败也要有反馈：状态条 + 日志写明原因（原来是"没反应"）
        let errs = self.cfg.validate();
        if !errs.is_empty() {
            let joined = errs.join("；");
            self.feedback("err", format!("无法开始扫描：{joined}"));
            for e in errs {
                self.push_log("err", &e);
            }
            return;
        }
        // 不在这里回写 toml：那会让「自动开跑」的机器每次启动都重建一遍配置文件。
        // 配置落盘只由用户点「保存配置」负责。
        let cfg = self.cfg.clone();
        let (tx, rx) = channel();
        // 关键：这里**不做任何遍历/统计**——目录树可能是几万文件或网络盘，同步数一遍正是
        // 「点了开始先卡一下」的元凶。遍历交给 worker，界面立刻进入过渡态。
        self.starting = true;
        // 本轮从零开始：表格先清（不叠在上一轮结果上），之后每批数据实时进表
        self.items.clear();
        self.scan_out_dir.clear();
        self.follow_tail = true;
        self.feedback("info", format!("正在启动扫描：{}…（后台统计文件，稍候）", cfg.root_dir));
        let handle = crate::core::scanner::spawn_scan(cfg, tx);
        self.running = Running::Scan { rx, t0: Instant::now(), handle };
    }

    fn start_filter(&mut self) {
        if self.running() || self.starting {
            self.feedback("warn", "已在运行中，忽略重复点击");
            return;
        }
        self.cfg.resolve_out_dir(&self.proc_dir);
        let errs = self.cfg.validate();
        if !errs.is_empty() {
            let joined = errs.join("；");
            self.feedback("err", format!("无法开始筛选：{joined}"));
            for e in errs {
                self.push_log("err", &e);
            }
            return;
        }
        self.starting = true;
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        let rcfg = crate::core::auto_run::filter_cfg_from_cfg(&self.cfg, &app_dir);
        let (tx, rx) = channel();
        let handle = fengine::spawn_filter(rcfg, tx);
        // 同上：本轮从零开始
        self.filter_items.clear();
        self.follow_tail = true;
        let head = format!(
            "正在启动筛选：{}  类型「{}」  回传{}{}",
            self.cfg.root_dir,
            self.cfg.log_type,
            if self.cfg.enabled { "开" } else { "关" },
            if self.cfg.enabled && self.cfg.dry_run { "（dry-run）" } else { "" }
        );
        self.feedback("info", head);
        let _ = &self.proc_dir;
        self.running = Running::Filter {
            handle,
            rx,
            t0: Instant::now(),
            up_index: 0,
            up_total: 0,
            up_pct: 0.0,
            sn: String::new(),
            status: String::new(),
        };
    }

    fn stop(&mut self) {
        match &self.running {
            Running::Filter { handle, .. } => handle.cancel.store(true, Ordering::SeqCst),
            Running::Scan { handle, .. } => handle.cancel(),
            Running::Idle => {}
        }
        let tail = if self.mode == WorkMode::Filter {
            // 筛选中途停止不写产物（回传流程被中断，产物不完整）；数据都在表格里，一键导出即可留档
            "已请求停止：正在收尾（结果保留在表格里，点「导出当前数据」可落盘）"
        } else {
            "已请求停止：正在收尾（已完成的部分会导出到输出目录）"
        };
        self.feedback("warn", tail);
    }

    // ---------- 一键导出 / 打开输出目录 ----------

    /// 把表格里现有的数据写成正式产物：中途停止、或想单独留档时用。
    /// 走的是和正常跑完全一样的那条产物写出路径（批次目录 + Excel + 审计 CSV）。
    /// **在后台线程里写**：几万行的 Excel 可能写好几秒，压在 UI 线程上就是「点一下导出就卡死」。
    fn export_now(&mut self) {
        if self.job_rx.is_some() {
            self.feedback("warn", "上一个后台任务（导出/打开目录）还没结束，稍等再点");
            return;
        }
        let is_filter = self.mode == WorkMode::Filter;
        let n = if is_filter { self.filter_items.len() } else { self.items.len() };
        if n == 0 {
            self.feedback("warn", "表格里还没有数据，先跑一次（或等结果出现）再导出");
            return;
        }
        self.cfg.resolve_out_dir(&self.proc_dir);
        let cfg = self.cfg.clone();
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        let items = self.items.clone();
        let rows = self.filter_items.clone();
        let fsum = self.summary.clone();
        let ssum = self.scan_summary.clone();
        self.spawn_job("正在导出当前数据", move || {
            if is_filter {
                let fc = crate::core::auto_run::filter_cfg_from_cfg(&cfg, &app_dir);
                // 用这次运行的真实耗时回推 t0：导出的「耗时(秒)」和跑完时是同一个数
                let t0 = Instant::now() - Duration::from_secs_f64(fsum.elapsed.max(0.0));
                match fengine::export_products(&fc, &rows, &fsum, t0) {
                    Ok((dir, _excel, _audit, _kept)) => (format!("已导出 {} 条 → {dir}", rows.len()), dir),
                    Err(e) => (format!("导出失败：{e}"), String::new()),
                }
            } else {
                match crate::core::scanner::export_scan_products(&items, &ssum, &cfg) {
                    Ok((dir, _xlsx, _copied)) => (format!("已导出 {} 条 → {dir}", items.len()), dir),
                    Err(e) => (format!("导出失败：{e}"), String::new()),
                }
            }
        });
    }

    /// 起一个后台任务：UI 线程立即返回，结果通过 job_rx 回来（导出 Excel、建目录、唤 explorer 都不许压在 UI 线程上）
    fn spawn_job<F>(&mut self, desc: &str, f: F)
    where
        F: FnOnce() -> (String, String) + Send + 'static,
    {
        let (tx, rx) = channel();
        self.job_rx = Some(rx);
        self.feedback("info", format!("{desc}…（后台进行，界面不会卡）"));
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    /// 后台任务收尾（每帧非阻塞看一眼）
    fn poll_job(&mut self) {
        let Some(rx) = &self.job_rx else { return };
        match rx.try_recv() {
            Ok((msg, dir)) => {
                let bad = msg.contains("失败") || msg.contains("打不开") || msg.contains("无法");
                if !dir.is_empty() {
                    if self.mode == WorkMode::Filter {
                        self.summary.batch_dir = dir;
                    } else {
                        self.scan_out_dir = dir;
                    }
                }
                self.feedback(if bad { "err" } else { "ok" }, msg);
                self.job_rx = None;
                self.need_repaint = true;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // 还在跑：让界面继续转，状态条上的进度条会动
                self.need_repaint = true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job_rx = None;
            }
        }
    }

    /// 按 rel_path 就地更新：同一份日志只留一行，状态类字段直接改这一行（etest 那种状态刷新）
    fn upsert_filter_row(&mut self, row: &Map<String, Value>) {
        let key = row.get("rel_path").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        for slot in self.filter_items.iter_mut() {
            if slot.get("rel_path").and_then(|v| v.as_str()).unwrap_or_default() == key {
                *slot = row.clone();
                return;
            }
        }
        self.filter_items.push(row.clone());
    }


    /// 「打开输出目录」的真实目标：优先本次产物的批次目录，其次配置的输出根
    fn out_dir_for_open(&self) -> String {
        if !self.summary.batch_dir.is_empty() {
            return self.summary.batch_dir.clone();
        }
        if !self.scan_out_dir.is_empty() {
            return self.scan_out_dir.clone();
        }
        self.cfg.out_dir.clone()
    }

    fn open_out_dir(&mut self) {
        let mut dir = self.out_dir_for_open();
        if dir.trim().is_empty() {
            self.cfg.resolve_out_dir(&self.proc_dir);
            dir = self.cfg.out_dir.clone();
        }
        if dir.trim().is_empty() {
            self.feedback("err", "输出目录未配置：先在「输出」里选一个目录");
            return;
        }
        // 建目录 + 唤 explorer 都甩后台：输出目录在网络盘上时，create_dir_all 一样会阻塞 UI
        self.spawn_job("正在打开输出目录", move || {
            // 还没跑过时目录可能不存在：先建出来再打开，否则 explorer 会打开别的地方（看着像「映射不对」）
            if !std::path::Path::new(&dir).exists() {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    return (format!("输出目录不存在且无法创建：{dir}（{e}）"), String::new());
                }
            }
            if open_path(&dir) {
                (format!("已打开输出目录：{dir}"), String::new())
            } else {
                (format!("打不开目录：{dir}"), String::new())
            }
        });
    }

    // ---------- 事件轮询 ----------

    fn poll(&mut self) {
        enum Shot {
            Scan(Box<crate::core::scanner::ScanOutcome>, String, String, usize),
            Filter(Box<FilterOutcome>, String),
            None,
        }
        // 单帧事件预算：数据量大时通道里可能堆着几万条事件，一帧全消费会把界面卡死；
        // 超预算就留到下一帧（try_recv 队列不会丢事件）。
        let budget = Instant::now() + Duration::from_millis(6);
        const MAX_EVENTS_PER_FRAME: usize = 2000;
        let mut consumed = 0usize;

        // 1) 扫描：批事件 append（表格实时长），Done 收尾
        let scan_shot = if let Running::Scan { rx, .. } = &mut self.running {
            let mut done = None;
            let mut pending: Vec<Box<crate::core::scanner::ScanOutcome>> = Vec::new();
            loop {
                if consumed >= MAX_EVENTS_PER_FRAME || Instant::now() > budget {
                    self.events_backlogged = true;
                    break;
                }
                match rx.try_recv() {
                    Ok(crate::core::scanner::ScanEvent::Batch(batch)) => {
                        consumed += 1;
                        pending.push(batch);
                    }
                    Ok(crate::core::scanner::ScanEvent::Done(o, dir, xlsx, copied)) => {
                        consumed += 1;
                        done = Some((o, dir, xlsx, copied));
                    }
                    Err(_) => break,
                }
            }
            for b in pending {
                self.items.extend(b.items);
            }
            if self.starting && !self.items.is_empty() {
                self.starting = false;
                self.feedback("info", format!("扫描进行中：已出 {} 行", self.items.len()));
            }
            done
        } else {
            None
        };

        // 2) 筛选：批事件 append，Log/Upload/Done 各归其位
        let filter_shot = if let Running::Filter { rx, .. } = &mut self.running {
            let mut evs: Vec<FilterEvent> = Vec::new();
            loop {
                if consumed >= MAX_EVENTS_PER_FRAME || Instant::now() > budget {
                    self.events_backlogged = true;
                    break;
                }
                match rx.try_recv() {
                    Ok(ev) => {
                        consumed += 1;
                        evs.push(ev);
                    }
                    Err(_) => break,
                }
            }
            let mut done = None;
            for ev in evs {
                match ev {
                    FilterEvent::Log(lvl, msg) => self.push_log(&lvl, &msg),
                    FilterEvent::Progress { .. } => {}
                    FilterEvent::Batch(batch) => {
                        self.filter_items.extend(batch.items);
                    }
                    FilterEvent::Row(row) => self.upsert_filter_row(&row),
                    FilterEvent::Upload { index, total, sn, status, pct } => {
                        if let Running::Filter { up_index, up_total, up_pct, sn: s, status: st, .. } = &mut self.running {
                            *up_index = index;
                            *up_total = total;
                            *up_pct = pct;
                            *s = sn;
                            *st = status;
                        }
                    }
                    FilterEvent::Done(outcome, note) => done = Some((outcome, note)),
                }
            }
            done
        } else {
            None
        };

        let shot = match (scan_shot, filter_shot) {
            (Some((o, dir, xlsx, copied)), _) => Shot::Scan(o, dir, xlsx, copied),
            (_, Some((o, note))) => Shot::Filter(o, note),
            _ => Shot::None,
        };

        match shot {
            Shot::Scan(outcome, batch_dir, xlsx, copied) => {
                self.scan_summary = outcome.summary.clone();
                self.items = outcome.items;
                self.push_log(
                    "ok",
                    &format!(
                        "完成：命中 {} / 未命中 {} / 跳过 {}，耗时 {}s",
                        self.scan_summary.hit, self.scan_summary.miss, self.scan_summary.skipped, self.scan_summary.elapsed
                    ),
                );
                let target = if batch_dir.is_empty() {
                    String::new()
                } else {
                    format!("{batch_dir}  ->  {xlsx}")
                };
                self.push_log("info", &format!("输出：{target}"));
                if copied > 0 {
                    self.push_log("ok", &format!("落盘命中文件 {copied} 份"));
                }
                self.starting = false;
                // 真实目录（不是「目录 -> 文件」这种展示串）：界面「打开输出目录」要能真的打开它
                self.scan_out_dir = batch_dir.clone();
                // 验收输出：R<{content,status,opts}>R（扫描模式——有命中即通过）
                let ok = self.scan_summary.hit > 0;
                let content = format!(
                    "扫描 命中 {}/{}（未命中 {}，跳过 {}），耗时 {}s，产物 {}",
                    self.scan_summary.hit, self.scan_summary.total_files, self.scan_summary.miss, self.scan_summary.skipped, self.scan_summary.elapsed, target
                );
                crate::core::result::emit(&content, ok, "scan");
                let mark = if ok { "PASS" } else { "FAIL" };
                self.feedback(if ok { "ok" } else { "warn" }, format!("扫描完成（{mark}）：{content}"));
                self.finish_ui(false);
            }
            Shot::Filter(outcome, note) => {
                // 收尾也走就地归并：表里已经是这些行，只更新字段（不再整表替换 -> 不会出现「最后清空重建」）
                for r in &outcome.items {
                    self.upsert_filter_row(r);
                }
                self.summary = outcome.summary.clone();
                let line = {
                    let s = &self.summary;
                    format!(
                        "筛选完成：提取 {}/{}（未知类型 {}），回传 成功 {} / 冲突 {} / 失败 {}{}",
                        s.extracted, s.total, s.unknown, s.upload_ok, s.upload_conflict, s.upload_fail,
                        if s.upload_dry > 0 { format!(" / dry-run {}", s.upload_dry) } else { String::new() }
                    )
                };
                self.push_log("ok", &line);
                // 拦截（都不判 PASS、不倒计时不关，自动化跑歪了必须有人看到）：
                //  ① 数据为空；② 开了正式回传却没真的回传（定向类型一台没匹配到 / 一台都没回传 / 有失败冲突）
                // ② 的判定与 R 结论共用 upload_anomaly，界面与验收输出不会各说一套。
                let empty = self.summary.total == 0 || self.summary.extracted == 0;
                if empty {
                    self.push_log(
                        "err",
                        &format!("数据为空拦截：目录 {} 内可处理日志为 0（文件 {}，提取 {}），程序保持打开", self.cfg.root_dir, self.summary.total, self.summary.extracted),
                    );
                    self.finish_ui(false);
                } else if let Some(why) = fengine::upload_anomaly(&self.summary, self.cfg.enabled, self.cfg.dry_run) {
                    self.push_log("err", &format!("回传拦截：{why}；程序保持打开待处理"));
                    self.finish_ui(false);
                } else {
                    let _ = note;
                    self.finish_ui(true);
                }
                // 验收输出：R<{content,status,opts}>R（与 --auto 同一套结论）
                self.starting = false;
                let (content, ok) = fengine::filter_verdict(&self.summary, self.cfg.enabled, self.cfg.dry_run);
                crate::core::result::emit(&content, ok, "filter");
                let mark = if ok { "PASS" } else { "FAIL" };
                self.feedback(if ok { "ok" } else { "err" }, format!("筛选完成（{mark}）：{content}"));
            }
            Shot::None => {}
        }

        if self.running() {
            self.need_repaint = true;
        }
    }

    fn finish_ui(&mut self, allow_countdown: bool) {
        self.running = Running::Idle;
        self.last_tick = None;
        let empty = self.summary.total == 0 || self.summary.extracted == 0;
        let upload_on = self.cfg.enabled && !self.cfg.dry_run;
        let ran = (self.summary.upload_ok + self.summary.upload_conflict + self.summary.upload_fail) > 0;
        let all_ok = ran && self.summary.upload_fail == 0 && self.summary.upload_conflict == 0;
        if self.mode == WorkMode::Filter {
            if self.cfg.auto_close && allow_countdown && !empty && !(upload_on && ran && !all_ok) {
                self.countdown = Some(self.cfg.countdown_sec.max(1) as u32);
            }
        } else {
            self.countdown = Some(0); // 扫描完成只弹提示
        }
    }

    fn tick_countdown(&mut self) {
        let Some(sec) = self.countdown else { return };
        if sec == 0 || self.mode != WorkMode::Filter || !self.cfg.auto_close {
            self.last_tick = None;
            return;
        }
        match self.last_tick {
            None => self.last_tick = Some(Instant::now()),
            Some(t) if t.elapsed() >= Duration::from_secs(1) => {
                self.last_tick = Some(Instant::now());
                let next = sec.saturating_sub(1);
                self.countdown = Some(next);
                if next == 0 {
                    self.push_log("info", "倒计时归零，自动关闭程序");
                    self.auto_close_window = true;
                }
            }
            _ => {}
        }
    }

    // ---------- 顶栏 ----------

    fn top_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("findany").size(SIZE_TITLE).strong().color(p.brand));
            ui.label(theme::dim(format!("v{}", env!("CARGO_PKG_VERSION")), p));
            ui.add_space(6.0);
            // 工作模式
            egui::ComboBox::from_id_salt("work_mode")
                .width(230.0)
                .selected_text(if self.mode == WorkMode::Scan { "通用扫描（包含 / 不包含）" } else { "日志筛选 / 回传（etest 系）" })
                .show_ui(ui, |ui| {
                    let mut is_filter = self.mode == WorkMode::Filter;
                    if ui.selectable_value(&mut is_filter, false, "通用扫描（包含 / 不包含）").clicked() {
                        self.mode = WorkMode::Scan;
                        self.cfg.work_mode = "scan".into();
                        self.scan_out_dir.clear();
                    }
                    if ui.selectable_value(&mut is_filter, true, "日志筛选 / 回传（etest 系）").clicked() {
                        self.mode = WorkMode::Filter;
                        self.cfg.work_mode = "filter".into();
                    }
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 主按钮：开始 / 启动中 / 正在停止 / 停止 —— 顶栏固定宽度，不被左侧面板压缩
                self.start_button(ui, p);
                // 保存配置：固定尺寸 + 状态配色，悬停不改任何几何量（不会抖）
                let (label, accent) = match self.save_state {
                    SaveState::Saved => ("已保存 ✓", p.ok),
                    SaveState::Failed => ("保存失败", p.err),
                    SaveState::Idle => ("保存配置", p.brand),
                };
                let size = egui::vec2(118.0, BTN_H);
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                if ui.is_rect_visible(rect) {
                    let fill = match self.save_state {
                        SaveState::Idle => {
                            if resp.hovered() {
                                p.brand.gamma_multiply(0.20)
                            } else {
                                p.panel2
                            }
                        }
                        _ => accent.gamma_multiply(0.18),
                    };
                    ui.painter().rect(
                        rect,
                        egui::CornerRadius::same(theme::RADIUS),
                        fill,
                        egui::Stroke::new(1.0, accent),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(SIZE_BODY),
                        accent,
                    );
                }
                if resp.clicked() {
                    let ok = self.save_now();
                    self.mark_saved(ok);
                }
                // 同步状态：配置只在点「保存配置」时写回 toml（没有自动保存）
                if self.dirty_now {
                    ui.label(RichText::new("● 有改动，点「保存配置」写回").size(SIZE_SMALL).color(p.warn));
                } else {
                    ui.label(RichText::new("● 已同步 findany.toml").size(SIZE_SMALL).color(p.ok));
                }
                // 后台任务（导出/打开目录）进行中：顶栏转圈，一眼知道还在忙
                if self.job_rx.is_some() {
                    ui.spinner();
                }
                // 最近一次操作的结果就地显示在顶栏（就在你点「开始」的地方）：
                // 校验失败、启动中、停止、导出结果都在这里出字。
                // 以前只有左侧面板底部和日志里有，面板一滚下去就什么都看不到 —— 点了像没反应。
                let (text, lvl) = self.status.clone();
                if !text.is_empty() {
                    let color = match lvl {
                        "err" => p.err,
                        "warn" => p.warn,
                        "ok" => p.ok,
                        _ => p.text2,
                    };
                    let icon = match lvl {
                        "err" => "✖",
                        "warn" => "▲",
                        "ok" => "✔",
                        _ => "●",
                    };
                    ui.add(
                        egui::Label::new(RichText::new(format!("{icon} {}", truncate(&text, 96))).size(SIZE_SMALL).color(color))
                            .truncate(),
                    )
                    .on_hover_text(&text);
                }
            });
        });
    }

    /// 固定 168px 宽、位置在顶栏右端，永不因左侧面板收放或文字变化被压缩。
    fn start_button(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let size = egui::vec2(168.0, BTN_H + 4.0);
        if self.cancelling() {
            let _ = ui.add_enabled(
                false,
                egui::Button::new(RichText::new("正在停止…").size(SIZE_BODY).strong().color(p.warn))
                    .fill(p.warn.gamma_multiply(0.12))
                    .min_size(size),
            );
            return;
        }
        if self.starting {
            let btn = egui::Button::new(RichText::new("启动中…（点此取消）").size(SIZE_BODY).strong().color(p.warn))
                .fill(p.warn.gamma_multiply(0.15))
                .min_size(size);
            if ui.add(btn).clicked() {
                self.stop();
            }
            return;
        }
        if self.busy() {
            if ui
                .add(egui::Button::new(RichText::new("停止").size(SIZE_BODY).strong().color(p.err)).min_size(size))
                .clicked()
            {
                self.stop();
            }
            return;
        }
        let btn = egui::Button::new(RichText::new("开始").size(SIZE_BODY).strong().color(Color32::WHITE))
            .fill(p.brand)
            .min_size(size);
        if ui.add(btn).clicked() {
            if self.mode == WorkMode::Scan {
                self.start_scan();
            } else {
                self.start_filter();
            }
        }
    }

    // ---------- 左侧配置面板 ----------

    fn config_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if !self.panel_open {
            if ui.add(egui::Button::new(theme::body("展开", p)).min_size(egui::vec2(56.0, BTN_H))).clicked() {
                self.panel_open = true;
            }
            return;
        }
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        let _ = app_dir;

        // ---- 视图控制（都收在左侧，保证右侧只有「开始」「保存配置」）----
        ui.horizontal_wrapped(|ui| {
            if ui.add(egui::Button::new(theme::body(if self.panel_open { "收起面板" } else { "展开面板" }, p)).min_size(egui::vec2(0.0, BTN_H))).clicked() {
                self.panel_open = !self.panel_open;
            }
            if ui.add(egui::Button::new(theme::body(if self.dark { "浅色主题" } else { "深色主题" }, p)).min_size(egui::vec2(0.0, BTN_H))).clicked() {
                self.dark = !self.dark;
            }
            if ui.add(egui::Button::new(theme::body(if self.log_open { "收起日志" } else { "运行日志" }, p)).min_size(egui::vec2(0.0, BTN_H))).clicked() {
                self.log_open = !self.log_open;
            }
            if ui.add(egui::Button::new(theme::body("重新载入", p)).min_size(egui::vec2(0.0, BTN_H))).clicked() {
                let path = crate::core::logfilter::autoconfig::config_path();
                self.cfg = crate::core::logfilter::autoconfig::load_config(&path);
                self.snapshot = self.cfg.clone();
                self.feedback("ok", format!("已重新载入配置：{}", path.display()));
            }
        });
        ui.add_space(6.0);

        // ---- 扫描目标 ----
        group(ui, p, "扫描目标", "目录或单个文件均可", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.cfg.root_dir).desired_width(f32::INFINITY).hint_text("目录 / 文件路径"));
            });
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(theme::body("浏览目录…", p)).min_size(egui::vec2(96.0, BTN_H))).clicked() {
                    if let Some(d) = pick_folder() {
                        self.cfg.root_dir = d.to_string_lossy().to_string();
                    }
                }
                if ui.add(egui::Button::new(theme::body("浏览文件…", p)).min_size(egui::vec2(96.0, BTN_H))).clicked() {
                    if let Some(f) = pick_file() {
                        self.cfg.root_dir = f.to_string_lossy().to_string();
                    }
                }
                if ui.add(egui::Button::new(theme::body("打开目录", p)).min_size(egui::vec2(88.0, BTN_H))).clicked() {
                    let _ = open_path(&self.cfg.root_dir);
                }
            });
            // 一眼看出填的是「文件」还是「目录」——两种走的是不同分支，选错了要当场知道。
            // 关键：**绝不在 UI 线程做 stat**。扫描目标是 UNC/网络盘时，一次 stat 就是一次网络往返，
            // 每帧 stat 会把界面卡死（用户报的「启动中界面卡死」）。路径变了才发一次后台检测。
            let t = self.cfg.root_dir.trim().to_string();
            {
                let settled = self.root_kind.lock().map(|g| g.0 == t).unwrap_or(true);
                if !settled && !t.is_empty() {
                    if let Ok(mut g) = self.root_kind.lock() {
                        g.0 = t.clone();
                        g.1 = PathKind::Checking;
                    }
                    let slot = std::sync::Arc::clone(&self.root_kind);
                    let probe = t.clone();
                    std::thread::spawn(move || {
                        let k = path_kind(&probe);
                        if let Ok(mut g) = slot.lock() {
                            g.0 = probe;
                            g.1 = k;
                        }
                    });
                }
            }
            let kind = self.root_kind.lock().map(|g| g.1).unwrap_or(PathKind::Missing);
            let (txt, col) = if t.is_empty() {
                ("未选：填/选一个目录，也可以直接选单个文件".to_string(), p.text2)
            } else {
                match kind {
                    PathKind::File => ("识别：单个文件（只处理这一个，不走扩展名过滤）".to_string(), p.ok),
                    PathKind::Dir => (
                        format!("识别：目录（{}，按扩展名过滤）", if self.cfg.recursive { "含子目录" } else { "仅本层" }),
                        p.ok,
                    ),
                    PathKind::Checking => ("识别中…（网络盘可能要等一下）".to_string(), p.text2),
                    PathKind::Missing => (format!("识别：路径不存在 —— {t}"), p.err),
                }
            };
            ui.label(RichText::new(txt).size(SIZE_SMALL).color(col));
            ui.add_space(2.0);
            // ---- 文件类型过滤（只扫 log / txt 这类）----
            // 点标签就切换进 cfg.extensions（唯一的真源，写回 findany.toml）；「全部」= 空列表 = 不筛
            ui.horizontal_wrapped(|ui| {
                row_label(ui, p, "文件类型");
                let all_on = self.cfg.extensions.is_empty();
                if ui.selectable_label(all_on, RichText::new("全部").size(SIZE_BODY)).clicked() {
                    self.cfg.extensions.clear();
                }
                for t in ["log", "txt", "csv", "json", "xml", "md", "ini"] {
                    let on = self.cfg.extensions.iter().any(|e| e.eq_ignore_ascii_case(t));
                    if ui.selectable_label(on, RichText::new(t).size(SIZE_BODY)).clicked() {
                        if on {
                            self.cfg.extensions.retain(|e| !e.eq_ignore_ascii_case(t));
                        } else {
                            self.cfg.extensions.push(t.to_string());
                            self.cfg.extensions.sort();
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                row_label(ui, p, "自定义");
                let mut exts = self.cfg.extensions.join(",");
                if ui
                    .add(egui::TextEdit::singleline(&mut exts).desired_width(f32::INFINITY).hint_text("留空=全部；逗号分隔，如 log,txt"))
                    .changed()
                {
                    self.cfg.extensions = exts.split(',').map(|s| s.trim().trim_start_matches('.').to_string()).filter(|s| !s.is_empty()).collect();
                }
            });
            ui.label(theme::dim(if self.cfg.extensions.is_empty() { "当前：全部文件（不按类型过滤）".to_string() } else { format!("当前：只处理 {} 这些类型", self.cfg.extensions.join(" / ")) }, p));
        });

        // ---- 自动运行（左侧独立成组，一眼能看到）----
        group(ui, p, "自动运行", "改完点「保存配置」写回 findany.toml", |ui| {
            let on = self.cfg.auto_start;
            let mut enable = on;
            // 只在「指针点击」时才接受变更：避免窗口获得焦点时被合成事件/键盘误翻开关
            let resp = ui
                .add(egui::Checkbox::new(&mut enable, RichText::new("启动后自动运行").size(SIZE_BODY).strong()))
                .on_hover_text("勾上后：下次双击打开程序即自动开始（按当前工作模式跑扫描或筛选），跑完按倒计时自动关");
            let user_clicked = resp.clicked() && ui.input(|i| i.pointer.any_click());
            if user_clicked && enable != on {
                self.cfg.auto_start = enable;
                if enable {
                    let mode_txt = if self.mode == WorkMode::Scan { "扫描" } else { "筛选 / 回传" };
                    self.feedback("warn", format!("已开启「启动后自动运行」：下次打开程序将自动{mode_txt}（配置已写入 findany.toml）"));
                } else {
                    self.feedback("info", "已关闭「启动后自动运行」");
                }
            }
            if on {
                ui.horizontal(|ui| {
                    row_label(ui, p, "完成后倒计时");
                    ui.add(egui::DragValue::new(&mut self.cfg.countdown_sec).range(3..=3600).suffix(" s"));
                    let mut close = self.cfg.auto_close;
                    if ui.checkbox(&mut close, RichText::new("归零自动关程序").size(SIZE_BODY)).changed() {
                        self.cfg.auto_close = close;
                    }
                });
                ui.label(theme::dim("下次启动生效；当前这轮不会自动开跑", p));
            } else {
                ui.label(theme::dim("当前关闭：打开程序后停在界面，点顶栏「开始」才跑", p));
            }
        });

        // ---- 通用扫描专属 ----
        if self.mode == WorkMode::Scan {
            group(ui, p, "检索条件", "仅通用扫描", |ui| {
                ui.horizontal(|ui| {
                    row_label(ui, p, "关键字");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.keyword).desired_width(f32::INFINITY));
                });
                ui.horizontal(|ui| {
                    row_label(ui, p, "匹配模式");
                    let inc = self.cfg.mode == "inc";
                    if ui.add(egui::Button::new(theme::body("包含", p)).selected(inc).min_size(egui::vec2(72.0, BTN_H))).clicked() {
                        self.cfg.mode = "inc".into();
                    }
                    if ui.add(egui::Button::new(theme::body("不包含", p)).selected(!inc).min_size(egui::vec2(72.0, BTN_H))).clicked() {
                        self.cfg.mode = "exc".into();
                    }
                    ui.checkbox(&mut self.cfg.case_sensitive, RichText::new("大小写敏感").size(SIZE_BODY));
                });
            });
        } else {
            // ---- 日志筛选 / 回传专属 ----
            let app_dir2 = self.proc_dir.to_string_lossy().to_string();
            group(ui, p, "日志筛选 / 回传", "etest 系", |ui| {
                ui.horizontal(|ui| {
                    row_label(ui, p, "筛选类型");
                    egui::ComboBox::from_id_salt("log_type")
                        .width(220.0)
                        .selected_text(match self.cfg.log_type.as_str() {
                            "auto" => "自动识别",
                            other => other,
                        })
                        .show_ui(ui, |ui| {
                            for (label, val) in [
                                ("自动识别", LogType::Auto.as_str()),
                                ("etest(OA3)", LogType::EtestOa3.as_str()),
                                ("etest", LogType::Etest.as_str()),
                                ("e-autotest", LogType::Eautotest.as_str()),
                                ("海格旧测试2", LogType::HegAutotest2.as_str()),
                                ("海格旧测试3", LogType::HegAutotest3.as_str()),
                            ] {
                                ui.selectable_value(&mut self.cfg.log_type, val.to_string(), label);
                            }
                        });
                });
                ui.checkbox(&mut self.cfg.enabled, RichText::new("启用数据回传（逐台调第三方 CLI）").size(SIZE_BODY));
                ui.checkbox(&mut self.cfg.dry_run, RichText::new("dry-run（只组包校验，不调 CLI）").size(SIZE_BODY));
                ui.checkbox(&mut self.cfg.auto_close, RichText::new("完成后倒计时自动关闭程序").size(SIZE_BODY));
                ui.horizontal(|ui| {
                    row_label(ui, p, "CLI 路径");
                    // 输入框不能吃掉全部宽度：右边还有个「…」选文件按钮，
                    // 以前按钮被顶出面板（用户看到的就是「文件按键看不全」）
                    let btn_w = 38.0 + ui.spacing().item_spacing.x;
                    let field_w = (ui.available_width() - btn_w).max(80.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.cfg.cli_path)
                            .desired_width(field_w)
                            .hint_text("留空=程序目录 intunehelper_cli.exe"),
                    );
                    if ui.add(egui::Button::new(theme::body("…", p)).min_size(egui::vec2(38.0, BTN_H))).clicked() {
                        if let Some(f) = pick_file() {
                            self.cfg.cli_path = f.to_string_lossy().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    row_label(ui, p, "SecretKey");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.secret_key).password(true).desired_width(f32::INFINITY));
                });
                ui.horizontal(|ui| {
                    row_label(ui, p, "参数模板");
                    let te = egui::TextEdit::singleline(&mut self.cfg.args).desired_width(f32::INFINITY).hint_text("~secret_key~ / ~payload~ / ~sn~");
                    ui.add(te).on_hover_text("占位符 ~key~：~secret_key~ / ~payload~ / 提取字段。含 ~payload~ 时走参数传 JSON，否则 payload 写 stdin");
                });
                let _ = app_dir2;
                ui.horizontal(|ui| {
                    row_label(ui, p, "超时");
                    ui.add(egui::DragValue::new(&mut self.cfg.timeout_sec).speed(1.0).range(5.0..=600.0).suffix(" s"));
                    row_label(ui, p, "重试");
                    ui.add(egui::DragValue::new(&mut self.cfg.max_retries).range(0..=10));
                    row_label(ui, p, "倒计时");
                    ui.add(egui::DragValue::new(&mut self.cfg.countdown_sec).range(3..=3600).suffix(" s"));
                });
            });
        }

        // ---- 运行参数 ----
        group(ui, p, "运行参数", "", |ui| {
            ui.horizontal(|ui| {
                row_label(ui, p, "并发线程");
                ui.add(egui::Slider::new(&mut self.cfg.threads, 1..=64).show_value(false));
                ui.add(egui::DragValue::new(&mut self.cfg.threads).range(1..=64));
            });
            // 窄面板（360px）下「编码」下拉 + 「超大跳过」一行放不下会把卡片顶出面板，所以可换行
            ui.horizontal_wrapped(|ui| {
                row_label(ui, p, "编码");
                egui::ComboBox::from_id_salt("enc")
                    .width(190.0)
                    .selected_text(match self.cfg.encoding.as_str() {
                        "auto" => "自动探测 (UTF-8 / GBK)",
                        "utf-8" => "UTF-8",
                        "gbk" => "GBK / GB2312",
                        "utf-16" => "UTF-16",
                        "ascii" => "ASCII",
                        other => other,
                    })
                    .show_ui(ui, |ui| {
                        for (label, val) in [
                            ("自动探测 (UTF-8 / GBK)", "auto"),
                            ("UTF-8", "utf-8"),
                            ("GBK / GB2312", "gbk"),
                            ("UTF-16", "utf-16"),
                            ("ASCII", "ascii"),
                        ] {
                            ui.selectable_value(&mut self.cfg.encoding, val.to_string(), label);
                        }
                    });
                row_label(ui, p, "超大跳过");
                ui.add(egui::DragValue::new(&mut self.cfg.max_file_mb).range(1.0..=2048.0).suffix(" MB"));
            });
            // 注意：上面这行是 horizontal_wrapped —— 面板窄到 360px 时「编码」下拉 + 「超大跳过」
            // 一行放不下会把整张卡片顶出面板（右边到顶），换行才是正确处置
            ui.horizontal(|ui| {
                row_label(ui, p, "界面刷新");
                let mut ms = self.cfg.ui_refresh_ms;
                let slider = egui::Slider::new(&mut ms, 0..=2000)
                    .suffix(" ms")
                    .custom_formatter(|v, _| if v == 0.0 { "仅结束时出表".to_string() } else { format!("{v:.0} ms") });
                if ui.add(slider).changed() {
                    self.cfg.ui_refresh_ms = ms;
                }
            })
            .response
            .on_hover_text("扫描/筛选时每隔多久把新结果推一批给表格：数值小=更实时、数值大=更省 CPU；0=只在结束时一次性出表");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.cfg.recursive, RichText::new("递归子目录").size(SIZE_BODY));
                let copy_label = if self.mode == WorkMode::Scan { "复制命中文件到 out" } else { "留存命中日志" };
                ui.checkbox(&mut self.cfg.copy_files, RichText::new(copy_label).size(SIZE_BODY));
                if self.mode == WorkMode::Scan {
                    ui.checkbox(&mut self.cfg.record_miss, RichText::new("Excel 记录未命中").size(SIZE_BODY));
                }
            });
            ui.separator();
            ui.label(theme::dim("资源控制（服务器上跑就靠这几项压住占用）", p));
            // 每批休眠 + 文件数上限：服务器上跑时的让路与保险丝
            ui.horizontal_wrapped(|ui| {
                row_label(ui, p, "每批休眠");
                ui.add(egui::DragValue::new(&mut self.cfg.throttle_ms).range(0..=5000).suffix(" ms"),
                )
                .on_hover_text("每处理完一批就睡这么久：给 CPU / 磁盘 / 网络盘让路（服务器建议 20~200ms，0=不让）");
                row_label(ui, p, "文件数上限");
                let mut cap = self.cfg.max_files;
                let dv = egui::DragValue::new(&mut cap)
                    .range(0..=100_000_000)
                    .custom_formatter(|v, _| if v == 0.0 { "不限".to_string() } else { format!("{v:.0}") });
                if ui.add(dv).changed() {
                    self.cfg.max_files = cap;
                }
            })
            .response
            .on_hover_text("最多处理多少个文件，0=不限：防目录跑飞的保险丝");
            ui.horizontal_wrapped(|ui| {
                row_label(ui, p, "进程优先级");
                egui::ComboBox::from_id_salt("prio")
                    .width(150.0)
                    .selected_text(match self.cfg.process_priority.as_str() {
                        "below_normal" => "低于正常",
                        "idle" => "最低（idle）",
                        _ => "正常",
                    })
                    .show_ui(ui, |ui| {
                        for (label, val) in [("正常", "normal"), ("低于正常", "below_normal"), ("最低（idle）", "idle")] {
                            ui.selectable_value(&mut self.cfg.process_priority, val.to_string(), label);
                        }
                    });
                ui.label(theme::dim("重启程序后生效（仅 Windows）", p));
            });
        });

        // ---- 输出 ----
        group(ui, p, "输出", "批次目录 out/<时间>/", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.cfg.out_dir).desired_width(f32::INFINITY).hint_text("留空=程序目录 out/"));
            });
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(theme::body("浏览…", p)).min_size(egui::vec2(80.0, BTN_H))).clicked() {
                    if let Some(d) = pick_folder() {
                        self.cfg.out_dir = d.to_string_lossy().to_string();
                    }
                }
                if ui.add(egui::Button::new(theme::body("打开输出目录", p)).min_size(egui::vec2(120.0, BTN_H))).clicked() {
                    self.open_out_dir();
                }
            });
        });

        // ---- 动作 ----
        // 操作反馈条：任何操作（点击/校验失败/启动中/完成/失败）都在这里出字，带级别配色
        {
            let (text, lvl) = self.status.clone();
            let color = match lvl {
                "err" => p.err,
                "warn" => p.warn,
                "ok" => p.ok,
                _ => p.text2,
            };
            let icon = match lvl {
                "err" => "✖",
                "warn" => "▲",
                "ok" => "✔",
                _ => "●",
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon).size(SIZE_BODY).color(color));
                ui.label(RichText::new(&text).size(SIZE_SMALL).color(color));
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // 开始/停止已移到顶栏（固定宽度，不被面板压缩）；这里只留次要动作
            if ui.add(egui::Button::new(theme::body("打开程序目录", p)).min_size(egui::vec2(130.0, BTN_H + 2.0))).clicked() {
                let _ = open_path(&self.proc_dir.to_string_lossy());
            }
        });
        ui.add_space(4.0);
        ui.label(theme::dim(format!("程序目录：{}", self.proc_dir.display()), p));
        if !self.font_note.is_empty() {
            ui.label(theme::dim(format!("中文字体：{}", self.font_note), p));
        }
    }
}

impl FindanyApp {
    // ---------- 右侧结果区 ----------

    fn result_area(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // 实时计数从共享句柄读（不受分批节流影响，进度条与统计数字始终跟手）
        let (live_done, live_total, live_pct, live_hit, live_miss, live_skip, running_secs) = match &self.running {
            Running::Scan { t0, handle, .. } => {
                let p: LiveProgress = handle.progress();
                (p.done, p.total, p.pct, p.hit, p.miss, p.skipped, t0.elapsed().as_secs_f64())
            }
            Running::Filter { t0, handle, .. } => {
                let p = handle.progress();
                (p.done, p.total, p.pct, p.extracted, p.unknown, p.skipped, t0.elapsed().as_secs_f64())
            }
            Running::Idle => (0, 0, 0.0, 0, 0, 0, 0.0),
        };
        let is_filter = self.mode == WorkMode::Filter;
        let running = self.running();
        let (a, b, c) = if running {
            (live_hit, live_miss, live_skip)
        } else if is_filter {
            (self.summary.extracted, self.summary.unknown, self.summary.skipped)
        } else {
            (self.scan_summary.hit, self.scan_summary.miss, self.scan_summary.skipped)
        };
        let elapsed = if self.scan_summary.elapsed > 0.0 { self.scan_summary.elapsed } else { self.summary.elapsed };

        ui.horizontal(|ui| {
            stat(ui, p, if is_filter { "提取成功" } else { "命中" }, &a.to_string(), p.ok);
            stat(ui, p, if is_filter { "未知类型" } else { "未命中" }, &b.to_string(), p.text2);
            stat(ui, p, "跳过", &c.to_string(), p.warn);
            let total_files = if running {
                live_total
            } else if is_filter {
                self.summary.total
            } else {
                self.scan_summary.total_files
            };
            // 遍历阶段总数还是 0：显式提示「正在统计」，避免看着像卡住
            let total_txt = if running && total_files == 0 { "统计中…".to_string() } else { total_files.to_string() };
            stat(ui, p, "文件总数", &total_txt, p.text);
            stat(ui, p, if running { "已用时" } else { "耗时" }, &format!("{:.2}s", if running { running_secs } else { elapsed }), p.info);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Running::Filter { up_total, up_index, up_pct, sn, status, .. } = &self.running {
                    if *up_total > 0 {
                        ui.label(theme::dim(format!("回传 {}/{}  {}  {}", up_index, up_total, sn, status), p));
                        ui.add(egui::ProgressBar::new((*up_pct as f32 / 100.0).clamp(0.0, 1.0)).desired_width(140.0));
                    }
                }
                let bar = if live_total > 0 { format!("{live_done}/{live_total}") } else { String::new() };
                ui.add(egui::ProgressBar::new((live_pct as f32 / 100.0).clamp(0.0, 1.0)).desired_width(180.0).text(bar));
            });
        });
        ui.add_space(6.0);
        // 结果表跟随最新一行（运行中默认开；用户自己拖滚动条就自动关掉，不抢操作）
        ui.horizontal(|ui| {
            let mut follow = self.follow_tail;
            if ui.checkbox(&mut follow, RichText::new("跟随最新（自动滚到底）").size(SIZE_SMALL)).changed() {
                self.follow_tail = follow;
            }
            ui.label(theme::dim(
                if self.running() {
                    "运行中：表格边跑边增长"
                } else {
                    "结束后可自由滚动/排序"
                },
                p,
            ));
            // 中途停止 / 想留档时：把表格里现有的数据直接落盘（走的还是正式产物那条路）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = if is_filter { self.filter_items.len() } else { self.items.len() };
                let tip = if n == 0 {
                    "表格里还没有数据"
                } else {
                    "把当前表格里的数据写成产物（批次目录 + Excel + 审计 CSV），中途停止也能留档"
                };
                if ui
                    .add_enabled(n > 0, egui::Button::new(theme::body("导出当前数据", p)).min_size(egui::vec2(130.0, BTN_H)))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.export_now();
                }
                if ui.add(egui::Button::new(theme::body("打开输出目录", p)).min_size(egui::vec2(130.0, BTN_H))).clicked() {
                    self.open_out_dir();
                }
            });
        });
        ui.separator();
        if is_filter {
            if self.cfg.enabled {
                let s = &self.summary;
                let ran = (s.upload_ok + s.upload_conflict + s.upload_fail) > 0;
                let txt = if !self.running() && ran {
                    format!("回传结果：成功 {} / 冲突 {} / 失败 {}{}", s.upload_ok, s.upload_conflict, s.upload_fail, if s.upload_dry > 0 { format!(" / dry-run {}", s.upload_dry) } else { String::new() })
                } else if self.running() {
                    "回传进行中…".to_string()
                } else {
                    format!("回传：{}", if self.cfg.dry_run { "dry-run 演练" } else { "等待开始" })
                };
                let color = if !self.running() && ran && s.upload_fail == 0 && s.upload_conflict == 0 { p.ok } else if s.upload_fail > 0 { p.err } else { p.text2 };
                ui.label(RichText::new(txt).size(SIZE_SMALL).color(color));
            }
            let changed = self.rows_changed;
            filter_table(ui, p, &self.filter_items, self.follow_tail, "filter_table", changed);
        } else {
            let changed = self.rows_changed;
            scan_table(ui, p, &self.items, self.follow_tail, "scan_table", changed);
        }
    }

    // ---------- 底部日志条 ----------

    fn log_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let (lvl, msg) = self.logs.last().map(|(_, l, m)| (l.clone(), m.clone())).unwrap_or(("info".into(), "(无日志)".into()));
            let color = match lvl.as_str() {
                "err" => p.err,
                "warn" => p.warn,
                "ok" => p.ok,
                _ => p.info,
            };
            ui.label(RichText::new("●").size(SIZE_SMALL).color(color));
            ui.label(RichText::new(truncate(&msg, 150)).size(SIZE_SMALL).color(p.text2));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("清空").clicked() {
                    self.logs.clear();
                }
                if ui.small_button(if self.log_open { "收起" } else { "展开" }).clicked() {
                    self.log_open = !self.log_open;
                }
            });
        });
    }

    fn log_window(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.log_open {
            return;
        }
        let mut open = true;
        egui::Window::new("运行日志")
            .open(&mut open)
            .default_size([820.0, 420.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for (t, lvl, msg) in &self.logs {
                        let color = match lvl.as_str() {
                            "err" => p.err,
                            "warn" => p.warn,
                            "ok" => p.ok,
                            _ => p.text2,
                        };
                        ui.label(RichText::new(format!("{t} [{}] {msg}", lvl.to_uppercase())).size(SIZE_SMALL).color(color));
                    }
                });
            });
        if !open {
            self.log_open = false;
        }
    }

    fn countdown_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(sec) = self.countdown else { return };
        let mut close_now = false;
        let is_filter = self.mode == WorkMode::Filter;
        egui::Window::new("完成")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let title = if is_filter { "日志筛选完成" } else { "扫描完成" };
                let sub = if is_filter {
                    format!("产物：{}", self.summary.batch_dir)
                } else {
                    format!("命中 {}，未命中 {}，跳过 {}", self.scan_summary.hit, self.scan_summary.miss, self.scan_summary.skipped)
                };
                ui.label(RichText::new(title).size(SIZE_HEAD).strong());
                ui.label(RichText::new(sub).size(SIZE_SMALL).color(p.text2));
                if is_filter && self.cfg.auto_close && sec > 0 {
                    ui.label(RichText::new(format!("{sec} 秒后自动关闭程序")).size(SIZE_SMALL).color(p.warn));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(theme::body("打开输出目录", p)).min_size(egui::vec2(130.0, BTN_H))).clicked() {
                        self.open_out_dir();
                    }
                    if ui.add(egui::Button::new(theme::body("延时 30s", p)).min_size(egui::vec2(100.0, BTN_H))).clicked() {
                        self.countdown = Some(sec + 30);
                    }
                    let cancel_label = if sec == 0 { "关闭提示" } else { "取消自动关" };
                    if ui.add(egui::Button::new(theme::body(cancel_label, p)).min_size(egui::vec2(110.0, BTN_H))).clicked() {
                        if sec == 0 {
                            close_now = true;
                        } else {
                            self.countdown = None;
                        }
                    }
                });
            });
        if close_now {
            self.countdown = None;
        }
        if is_filter && self.cfg.auto_close && sec > 0 {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
    }

/// 行内标签（自由函数：避免与 self.cfg 的借用冲突）
fn row_label(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).size(SIZE_BODY).color(p.text2));
}

/// --uitest 的自证开关（每帧查环境变量太浪费，缓存一次）
fn uitest_debug() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("FINDANY_UITEST_DEBUG").is_ok())
}

/// 回归自证入口（--uitest 用）：造 3 行数据跑一次「导出当前数据」，等后台任务收尾，返回状态消息。
/// 证明「导出这种重活确实在后台线程跑、UI 线程只 try_recv」——卡死根治的那道闸门。
pub fn export_probe(app: &mut FindanyApp, dir: &std::path::Path) -> String {
    app.mode = WorkMode::Scan;
    app.cfg.out_dir = dir.to_string_lossy().to_string();
    app.items = (0..3)
        .map(|i| ScanItem {
            filename: format!("f{i}.log"),
            rel_path: format!("f{i}.log"),
            hit_line_text: "x".into(),
            encoding: "utf-8".into(),
            ..Default::default()
        })
        .collect();
    app.export_now();
    let mut waited = 0u32;
    while app.job_rx.is_some() && waited < 200 {
        std::thread::sleep(Duration::from_millis(25));
        app.poll_job();
        waited += 1;
    }
    app.status.0.clone()
}

/// 日志写线程：UI 线程只把批量日志丢进队列，落盘全在后台（UI 线程零 I/O）
fn make_log_writer() -> std::sync::mpsc::Sender<Vec<(String, String)>> {
    let (tx, rx) = channel::<Vec<(String, String)>>();
    std::thread::spawn(move || {
        while let Ok(batch) = rx.recv() {
            crate::core::app_dir::log_lines("findany-gui.log", &batch);
        }
    });
    tx
}

/// 扫描目标的类型（识别在后台线程做，UI 线程只读缓存结果）
#[derive(Clone, Copy, PartialEq)]
pub enum PathKind {
    Checking,
    File,
    Dir,
    Missing,
}

/// 判定路径类型（文件/目录/不存在）。**只能从后台线程调**：UNC/网络盘上这是阻塞的网络往返。
fn path_kind(p: &str) -> PathKind {
    let path = std::path::Path::new(p);
    if path.is_file() {
        PathKind::File
    } else if path.is_dir() {
        PathKind::Dir
    } else {
        PathKind::Missing
    }
}

/// 卡片式分组：标题 + 提示 + 内容
fn group(ui: &mut egui::Ui, p: &Palette, title: &str, hint: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            // 卡片右边界锁在面板内：标题+提示这一行超宽会把整张卡顶出去（右圆角/边框被面板切掉，
            // 用户看到的就是「右边到顶」）。所以标题行必须能换行，宽度也钉住。
            let w = ui.available_width();
            ui.set_max_width(w);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(title).size(SIZE_HEAD).strong().color(p.text));
                if !hint.is_empty() {
                    ui.label(RichText::new(hint).size(SIZE_SMALL).color(p.text2));
                }
            });
            ui.add_space(4.0);
            body(ui);
            if uitest_debug() {
                println!("[panel] 组「{title}」内容宽 {:.0} / 可用 {:.0}", ui.min_rect().width(), w);
            }
        });
    ui.add_space(8.0);
}
/// 回归自证入口（--uitest 用）：按给定宽度画一遍左侧面板，返回内容实际宽度。
/// 任何一行超宽（输入框没留按钮位置、标题+提示太长）都会把卡片/按钮顶出面板，
/// 用户看到的就是「按键看不全」「右边到顶」。
pub fn measure_config_panel(app: &mut FindanyApp, ui: &mut egui::Ui, width: f32) -> f32 {
    app.panel_open = true;
    let p = app.palette();
    let mut got = 0.0f32;
    ui.allocate_ui_with_layout(egui::vec2(width, 700.0), egui::Layout::top_down(egui::Align::Min), |ui| {
        app.config_panel(ui, &p);
        got = ui.min_rect().width();
    });
    got
}

/// 回归自证入口（--uitest 用）：铺满屏幕的 Area 里跑一段 UI（拿到根 Ui 的最简公开办法）
pub fn with_central_test_ui(ctx: &eframe::egui::Context, add: impl FnOnce(&mut egui::Ui)) {
    let rect = ctx.input(|i| i.viewport().inner_rect.unwrap_or(i.content_rect()));
    let min = rect.min;
    let size = if rect.width().is_finite() && rect.height().is_finite() {
        rect.size().max(egui::vec2(1.0, 1.0))
    } else {
        egui::vec2(1.0, 1.0)
    };
    egui::Area::new(egui::Id::new("uitest_root"))
        .fixed_pos(min)
        .show(ctx, |ui| {
            ui.set_min_size(size);
            // 夹成有限高度：Area 的 max_rect 是"要多大给多大"，表格视口会跟着变成无界，
            // 虚拟化/滚动范围就都不是真实几何了。真实界面里表格在 Panel 内，高度有界。
            ui.set_max_height(size.y);
            add(ui);
        });
}

/// 回归自证入口（--uitest 用）：用 egui 的 central 层画一遍表格，
/// 覆盖「首帧/0 尺寸/极小窗口」等历史上会喂出 NaN 的布局。
pub fn render_scan_table_for_test(ui: &mut egui::Ui, items: &[ScanItem], follow: bool) -> (f32, f32, f32, f32) {
    // 一帧只画一张表：表格在 Ui 里是竖着排的，画两张时第二张会被挤出可视区，
    // 布局会被 egui 裁剪掉，量到的几何就不是真实值了。
    // follow=true 走「本帧有新行 → 钉到底」那条分支（历史上正是在这里 NaN 崩溃）。
    let p = theme::dark();
    let salt = if follow { "uitest_table_follow" } else { "uitest_table" };
    scan_table(ui, &p, items, follow, salt, true)
}

fn stat(ui: &mut egui::Ui, p: &Palette, label: &str, value: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(SIZE_SMALL).color(p.text2));
        ui.label(RichText::new(value).size(SIZE_STAT).strong().color(color));
    });
    ui.add_space(16.0);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// 同步文件对话框（rfd 走异步 API，这里用一次性运行时阻塞取结果）
fn pick_folder() -> Option<std::path::PathBuf> {
    rt().block_on(e_utils::dialog::a_sync::folder())
}

fn pick_file() -> Option<std::path::PathBuf> {
    rt().block_on(e_utils::dialog::a_sync::file())
}

fn rt() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime"))
}

/// 打开目录；成功 true。失败必须让调用方反馈出来（不允许点了没反应）
fn open_path(path: &str) -> bool {
    if path.trim().is_empty() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("explorer")
            .arg(path.replace('/', "\\"))
            .spawn()
            .is_ok();
    }
    #[cfg(not(target_os = "windows"))]
    {
        return std::process::Command::new("xdg-open").arg(path).spawn().is_ok();
    }
}

fn ext(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_lowercase(),
        None => String::new(),
    }
}

/// 表格视口定高：egui_extras 用「可用高度」算可见行范围，不封顶它就会把全部行每帧画一遍
/// （几万行时正是界面卡死的根因）。这里用 Frame 夹出确定高度，虚拟化才真正生效。
fn table_viewport(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> egui::Vec2) -> egui::Vec2 {
    // 视口尺寸必须**确定且有限**：表格的表头/行布局用「可用尺寸」算列宽行高，
    // 拿到非有限值就会在 egui 的 debug_assert 上崩（崩溃 backtrace 已定位到
    // scan_table -> TableBuilder::header -> StripLayout::add -> Ui::set_min_height）。
    let sane = |v: f32, fallback: f32, min: f32| -> f32 {
        if v.is_finite() {
            v.max(min)
        } else {
            fallback
        }
    };
    let w = sane(ui.available_width(), 640.0, 120.0);
    let h = sane(ui.available_height() - 2.0, 320.0, 120.0);
    let size = egui::vec2(w, h);
    egui::Frame::new().show(ui, |ui| {
        ui.set_min_size(size);
        ui.set_max_size(size);
        let _ = add(ui);
    });
    size
}

#[allow(clippy::type_complexity)]
fn scan_table(ui: &mut egui::Ui, p: &Palette, items: &[ScanItem], follow: bool, scroll_id: &str, rows_changed: bool) -> (f32, f32, f32, f32) {
    let headers = ["序号", "相对路径", "目录", "扩展名", "包含状态", "命中行号", "命中行内容", "匹配计数", "大小", "修改时间", "编码"];
    // 供 --uitest 断言「跟随最新时每帧都精确贴底、且 offset 单调不减」
    let mut probe = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut rendered = 0usize;
    table_viewport(ui, |ui| {
        let vp = ui.max_rect().size();
        // 外层滚动区只管横向（列比窗口宽）；**纵向滚动必须用表格自己的滚动区**——
        // TableBuilder::body() 内部本身就是一个 ScrollArea（管行）。以前把纵向也交给外面这层，
        // 外面那层的内容高恰好等于一屏（表格自己的滚动区就那么大），纵向范围恒为 0：
        //   · 每批新行都会把整张表顶上去再被夹回来 -> 画面一闪一闪
        //   · 表格永远不跟随最新行 -> 用户看不到实时预览
        let sa = egui::ScrollArea::horizontal().id_salt(scroll_id).auto_shrink([false, false]);
        sa.show(ui, |ui| {
        let mut tb = egui_extras::TableBuilder::new(ui)
            .vscroll(true)
            // 程序化滚动立即生效：平滑追赶动画 + 每 200ms 一批的刷新率 = 肉眼看到的"来回追"
            .animate_scrolling(false)
            .auto_shrink([false, false]);
        if follow {
            tb = tb.stick_to_bottom(true);
            if rows_changed && !items.is_empty() {
                // 本帧有新行：把表格自己的滚动区钉到最后一行的底部
                tb = tb.scroll_to_row(items.len() - 1, Some(egui::Align::BOTTOM));
            }
        }
        let out = tb
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::exact(52.0))
            .column(egui_extras::Column::initial(280.0).at_least(140.0).clip(true))
            .column(egui_extras::Column::initial(130.0).at_least(70.0).clip(true))
            .column(egui_extras::Column::exact(66.0))
            .column(egui_extras::Column::exact(78.0))
            .column(egui_extras::Column::initial(120.0).clip(true))
            .column(egui_extras::Column::initial(340.0).clip(true))
            .column(egui_extras::Column::exact(76.0))
            .column(egui_extras::Column::exact(86.0))
            .column(egui_extras::Column::initial(160.0).clip(true))
            .column(egui_extras::Column::exact(96.0))
            .header(ROW_H, |mut header| {
                for h in headers {
                    header.col(|ui| {
                        ui.label(RichText::new(h).size(SIZE_SMALL).strong().color(p.text2));
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_H, items.len(), |mut row| {
                    rendered += 1;
                    let i = row.index();
                    let it = &items[i];
                    row.col(|ui| { ui.label(RichText::new((i + 1).to_string()).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(&it.rel_path).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(&it.dir_name).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(ext(&it.filename)).size(SIZE_BODY)); });
                    row.col(|ui| {
                        let (t, c) = if it.hit { ("命中", p.ok) } else { ("未命中", p.text2) };
                        ui.label(RichText::new(t).size(SIZE_BODY).color(c));
                    });
                    row.col(|ui| {
                        let s = if it.hit { it.hit_lines.iter().take(6).map(|x| x.to_string()).collect::<Vec<_>>().join(",") } else { String::new() };
                        ui.label(RichText::new(s).size(SIZE_BODY));
                    });
                    row.col(|ui| { ui.label(RichText::new(if it.hit { &it.hit_line_text } else { "" }).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(if it.hit { it.hit_count.to_string() } else { String::new() }).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(it.size_str()).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(it.mtime_str()).size(SIZE_BODY)); });
                    row.col(|ui| { ui.label(RichText::new(&it.encoding).size(SIZE_BODY)); });
                });
            });
        // 探针：--uitest 用「跟随最新时每帧精确贴底 + 位置单调不减」锁住这个行为
        probe = (out.state.offset.y, (out.content_size.y - out.inner_rect.height()).max(0.0), out.content_size.y, out.inner_rect.height());
        if uitest_debug() {
            println!(
                "[dbg] rows={} rendered={} vp={:?} offset={:?} content={:?} inner={:?}",
                items.len(), rendered, vp, out.state.offset, out.content_size, out.inner_rect.size()
            );
        }
        });
        vp
    });
    probe
}

#[allow(clippy::type_complexity)]
fn filter_table(ui: &mut egui::Ui, p: &Palette, items: &[Map<String, Value>], follow: bool, scroll_id: &str, rows_changed: bool) -> (f32, f32, f32, f32) {
    // 供 --uitest 断言「跟随最新时每帧都精确贴底、且 offset 单调不减」
    let mut probe = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let cols: [(&str, &str, f32); 14] = [
        ("序号", "idx", 52.0),
        ("文件", "log_file", 260.0),
        ("判型", "detected_type", 110.0),
        ("SN", "sn", 220.0),
        ("ProductKeyID", "product_key_id", 150.0),
        ("Hash长度", "hardware_hash_len", 86.0),
        ("Baseboard", "baseboard_product", 130.0),
        ("JSON状态", "json_state", 86.0),
        ("提取状态", "extract_state", 86.0),
        ("回传状态", "upload_state", 96.0),
        ("退出码", "upload_code", 74.0),
        ("request_id", "request_id", 170.0),
        ("提取错误", "error", 200.0),
        ("回传错误", "upload_error", 240.0),
    ];
    table_viewport(ui, |ui| {
        let vp = ui.max_rect().size();
        // 外层只管横向、纵向交给表格自己的滚动区（理由见 scan_table）
        let sa = egui::ScrollArea::horizontal().id_salt(scroll_id).auto_shrink([false, false]);
        sa.show(ui, |ui| {
        let mut tb = egui_extras::TableBuilder::new(ui)
            .vscroll(true)
            .animate_scrolling(false)
            .auto_shrink([false, false])
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for (_, _, w) in cols {
            tb = tb.column(egui_extras::Column::initial(w).at_least(54.0).clip(true));
        }
        if follow {
            tb = tb.stick_to_bottom(true);
            if rows_changed && !items.is_empty() {
                tb = tb.scroll_to_row(items.len() - 1, Some(egui::Align::BOTTOM));
            }
        }
        let out = tb.header(ROW_H, |mut header| {
            for (h, _, _) in cols {
                header.col(|ui| {
                    ui.label(RichText::new(h).size(SIZE_SMALL).strong().color(p.text2));
                });
            }
        })
        .body(|body| {
            body.rows(ROW_H, items.len(), |mut row| {
                let i = row.index();
                let it = &items[i];
                row.col(|ui| { ui.label(RichText::new((i + 1).to_string()).size(SIZE_BODY)); });
                for (_, key, _) in cols.iter().skip(1) {
                    row.col(|ui| {
                        let v = match it.get(*key) {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Null) | None => String::new(),
                            Some(other) => other.to_string(),
                        };
                        let color = match *key {
                            "upload_state" | "extract_state" => match v.as_str() {
                                "成功" => p.ok,
                                "失败" => p.err,
                                "冲突(人工)" => p.warn,
                                _ => p.text,
                            },
                            _ => p.text,
                        };
                        ui.label(RichText::new(truncate(&v, 64)).size(SIZE_BODY).color(color));
                    });
                }
            });
        });
        probe = (out.state.offset.y, (out.content_size.y - out.inner_rect.height()).max(0.0), out.content_size.y, out.inner_rect.height());
        });   // 收 sa.show
        vp
    });   // 收 table_viewport 闭包
    probe
}

impl eframe::App for FindanyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 帧级兜底：极少数情况下（首帧 / 极端布局）父级 max_rect 可能是非有限值，
        // 一旦它泄漏进 Panel / ScrollArea / TableBuilder，egui 的 debug_assert 就会崩。
        // 这里先把根 Ui 的约束夹成有限值，让整棵树都拿到干净数字。
        {
            let mr = ui.max_rect();
            let w_bad = !mr.width().is_finite();
            let h_bad = !mr.height().is_finite();
            if w_bad || h_bad {
                let w = if w_bad { 1024.0 } else { mr.width().max(1.0) };
                let h = if h_bad { 720.0 } else { mr.height().max(1.0) };
                ui.set_max_size(egui::vec2(w, h));
                ui.set_min_size(egui::vec2(w * 0.5, h * 0.5));
            }
        }
        let p = self.palette();
        theme::apply(&ctx, &p, self.dark);
        self.poll();
        // 本帧行数变化 -> 结果表抬底（只在有新内容那一帧抬，不跟用户抢滚动条）
        let rows_now = self.items.len() + self.filter_items.len();
        self.rows_changed = rows_now != self.last_row_count;
        self.last_row_count = rows_now;
        self.tick_save(&ctx);
        self.tick_countdown();
        if self.need_repaint {
            self.need_repaint = false;
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        // UI 线程登记一次：后台 I/O 辅助函数靠它把「在 UI 线程里做 I/O」当场断言出来
        crate::core::app_dir::mark_ui_thread();
        self.poll_job();
        egui::Panel::top("topbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.top_bar(ui, &p);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("logbar").show(ui, |ui| {
            self.log_bar(ui, &p);
        });
        let panel_w = if self.panel_open { PANEL_W } else { PANEL_W_FOLDED };
        egui::Panel::left("cfg")
            .resizable(self.panel_open)
            .default_size(panel_w)
            .size_range(Rangef::new(
                if self.panel_open { 360.0 } else { PANEL_W_FOLDED },
                if self.panel_open { 680.0 } else { PANEL_W_FOLDED },
            ))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    self.config_panel(ui, &p);
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            self.result_area(ui, &p);
        });
        self.log_window(&ctx, &p);
        self.countdown_window(&ctx, &p);

        // 自开始 / 自动化：首帧触发一次
        if (self.cfg.auto_start || self.auto_mode) && !self.running() && self.logs.len() <= 1 {
            self.push_log("info", if self.auto_mode { "自动化模式：启动即开跑（TOML）" } else { "run.auto_start=true，自动开跑" });
            if self.mode == WorkMode::Scan {
                self.start_scan();
            } else {
                self.start_filter();
            }
        }
        // 倒计时归零：关窗退出
        if self.auto_close_window {
            self.auto_close_window = false;
            // 关窗前把缓冲日志交给写线程，并给它一点时间落盘（UI 线程自己不写文件）
            self.flush_logs();
            std::thread::sleep(Duration::from_millis(80));
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// 启动 GUI（auto_mode=true 时首帧自动开跑并按 TOML 倒计时关窗）
pub fn run(cfg: SearchConfig, proc_dir: std::path::PathBuf, auto_mode: bool) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1040.0, 660.0])
            .with_title("findany — 目录内容扫描器"),
        ..Default::default()
    };
    eframe::run_native(
        "findany",
        native_options,
        Box::new(move |cc| {
            let font_note = theme::install_fonts(&cc.egui_ctx);
            Ok(Box::new(FindanyApp::new(cfg, proc_dir, font_note, auto_mode)))
        }),
    )
}
