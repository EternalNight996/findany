//! egui 主界面（1:1 对齐 Python 版 PySide6 布局与流程）：
//! 顶栏 / 左侧配置面板（440px，可收放）/ 右侧统计+结果表 / 底部日志条 / 完成倒计时。
//!
//! v2 约定：
//!   - 唯一配置文件 findany.toml；任何设置变动 500ms 后自动落盘（按钮另有即时反馈）
//!   - 整体字号走 theme::SIZE_*，不再硬编码
//!   - 已移除 SN 关联 / 单文件筛选方案（不需要）

use crate::core::config::SearchConfig;
use crate::core::logfilter::engine::{self as fengine, FilterEvent, FilterSummary};
use crate::core::logfilter::types::LogType;
use crate::core::scanner::ScanItem;
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
    /// **独立模式**：历史结果重传 —— 选目录、递归解析 filter_result.xlsx、逐文件回传。
    /// 不跑文件遍历/字段提取（数据来自历史产物），所以单独成一档，不与扫描/筛选混在一起。
    Retry,
}

impl WorkMode {
    /// 对应的 `cfg.work_mode` 字符串
    fn key(self) -> &'static str {
        match self {
            WorkMode::Scan => "scan",
            WorkMode::Filter => "filter",
            WorkMode::Retry => "retry",
        }
    }
    fn from_key(s: &str) -> Self {
        match s {
            "scan" => WorkMode::Scan,
            "retry" => WorkMode::Retry,
            _ => WorkMode::Filter,
        }
    }
    /// 顶栏下拉的显示名
    fn label(self) -> &'static str {
        match self {
            WorkMode::Scan => "通用扫描（包含 / 不包含）",
            WorkMode::Filter => "日志筛选 / 回传（etest 系）",
            WorkMode::Retry => "历史结果重传（解析 filter_result.xlsx）",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SaveState {
    Idle,
    Saved,
    Failed,
}

/// 模式数组下标（顺序与 `autoconfig::load_mode_configs` 一致：scan / filter / retry）
fn mode_idx(m: WorkMode) -> usize {
    match m {
        WorkMode::Scan => 0,
        WorkMode::Filter => 1,
        WorkMode::Retry => 2,
    }
}

/// 一个模式自己的**结果数据**（切模式时整份换过去）：
/// 表格行、行索引、缓存池、汇总、抬底状态 —— 都按模式独立，两个模式的结果不混在一张表里。
#[derive(Default)]
struct ModeData {
    items: Vec<Map<String, Value>>,
    index: std::collections::HashMap<String, usize>,
    pool: Option<std::sync::Arc<crate::core::lru_pool::RowPool<Map<String, Value>>>>,
    summary: FilterSummary,
    last_row_count: usize,
    rows_changed: bool,
    ui_truncated_note: bool,
    scan_out_dir: String,
}

enum Running {
    Idle,
    /// **两个模式共用同一个运行态**（通用扫描 / 日志筛选回传是同一条管道，
    /// 差异只在 `cfg.work_mode` 决定的 process_one 策略）。
    /// 之前分成 Scan/Filter 两个变体，导致事件类型、表格数据、池、统计全都要写两套。
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

/// 待主上裁决的「续跑询问」：点开始时读到上次未完成的任务节点，先问「继续上次 / 从头开始」。
struct ResumePrompt {
    /// 上次留下的进度节点（`<out_dir>/_resume.json`）
    node: crate::core::logfilter::resume::ResumeNode,
    /// 已经准备好的配置（选「继续」就直接拿它启动，不再重新组装）
    rcfg: fengine::FilterRunCfg,
    /// 本次要跑的模式是扫描吗（决定按钮文案）
    scan_mode: bool,
}

pub struct FindanyApp {
    cfg: SearchConfig,
    /// 三个模式各自的模版（顺序 scan / filter / retry）：左侧栏的数据（含扫描目标目录）
    /// 按模式独立，切模式互不覆盖。`cfg` 永远是「当前模式」那一份，切模式时与对应槽互换。
    cfg_modes: [SearchConfig; 3],
    /// 三个模式各自的结果数据（表格 / 汇总 / 缓存池）：切模式时整份换，互不串用
    mode_data: [ModeData; 3],
    /// 上次落盘时的配置快照（用于顶栏「有改动，点保存配置」提示）
    snapshot: SearchConfig,
    mode: WorkMode,
    dark: bool,
    panel_open: bool,
    running: Running,
    /// 待主上裁决的续跑询问（点开始时发现上次没跑完时挂起）
    resume_prompt: Option<ResumePrompt>,
    /// 表格行数据 —— **两个模式共用这一份**（通用扫描与日志筛选回传是同一条管道，
    /// 行结构也统一为 Map）。之前这里还有一份 `items: Vec<ScanItem>` 是扫描专用的，
    /// 正是"两套分离"的残留。
    filter_items: Vec<Map<String, Value>>,
    /// UI 端行缓存池：worker 推过来的行超过 cache_capacity_rows 时，把最旧的移到这里
    /// （表格底部「加载更多」可从池里拉回）。
    row_pool: Option<std::sync::Arc<crate::core::lru_pool::RowPool<Map<String, Value>>>>,
    /// 行索引（rel_path -> filter_items 下标）：upsert 从 O(N) 线性扫描降到 O(1)。
    /// **淘汰 / 头部插入后下标会整体偏移** ── 那些位置必须调 rebuild_filter_index()。
    filter_index: std::collections::HashMap<String, usize>,
    /// 两个模式共用的汇总（统一管道产出；扫描模式的命中/未命中也在 FilterSummary 里）
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
    /// 表格截断提示只出一次
    ui_truncated_note: bool,
    /// 日志写线程的队列（UI 线程只 send，不碰文件）
    log_tx: std::sync::mpsc::Sender<Vec<(String, String)>>,
    /// 后台任务（导出 / 打开目录）结果：(给人看的消息, 产物目录)。UI 线程只 try_recv
    job_rx: Option<Receiver<(String, String)>>,
    /// 后台任务开始时间（用于超时兜底：网络盘上 exists/create_dir_all 可能阻塞几十秒）
    job_started: Option<Instant>,
    scan_out_dir: String,
    font_note: String,
    proc_dir: std::path::PathBuf,
}

impl FindanyApp {
    pub fn new(cfg: SearchConfig, proc_dir: std::path::PathBuf, font_note: String, auto_mode: bool) -> Self {
        let start_mode = WorkMode::from_key(&cfg.work_mode);
        // 没显式装载三份模版时，三个模式先都拿到这一份（之后各自独立演化）
        let cfg_modes = [cfg.clone(), cfg.clone(), cfg.clone()];
        let mut logs = Vec::new();
        logs.push((
            chrono::Local::now().format("%H:%M:%S").to_string(),
            "info".to_string(),
            format!("配置文件：{}", crate::core::logfilter::autoconfig::config_path().display()),
        ));
        let app = Self {
            snapshot: cfg.clone(),
            cfg,
            cfg_modes,
            mode_data: Default::default(),
            mode: start_mode,
            dark: true,
            panel_open: true,
            running: Running::Idle,
            resume_prompt: None,
            filter_items: Vec::new(),
            row_pool: None,
            filter_index: std::collections::HashMap::new(),
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
            ui_truncated_note: false,
            log_pending: Vec::new(),
            log_flush_at: None,
            log_tx: make_log_writer(),
            job_rx: None,
            job_started: None,
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

    /// 把缓冲的日志交给写线程（最多每 250ms 一次；超过 69 条立刻投，别让日志迟到）。
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
            Running::Filter { handle, .. } => handle.cancel.load(Ordering::Relaxed),
            Running::Idle => false,
        }
    }

    // ---------- 配置：单一文件 findany.toml ----------

    fn cfg_dirty(&self) -> bool {
        serde_json::to_value(&self.cfg).ok() != serde_json::to_value(&self.snapshot).ok()
    }


    /// 载入三个模式的模版（启动时由 main 传进来；不调用则三份都等于当前 cfg）
    pub fn set_mode_cfgs(&mut self, modes: [SearchConfig; 3]) {
        self.cfg_modes = modes;
        // 当前模式这一份以 self.cfg 为准：顶层 [filter]/[upload]/[run] 就是它的镜像
        let i = mode_idx(self.mode);
        self.cfg_modes[i] = self.cfg.clone();
    }

    /// 把当前 cfg 存回它对应的模式槽（切模式 / 保存配置前都要做）
    fn store_mode_cfg(&mut self) {
        let i = mode_idx(self.mode);
        self.cfg_modes[i] = self.cfg.clone();
    }

    /// 切模式：当前这份先存回旧槽，再载入新槽 —— 左侧栏数据（含扫描目标目录）各模式独立
    fn switch_mode(&mut self, m: WorkMode) {
        if self.mode == m {
            return;
        }
        // 运行中不许切：worker 的结果是按「当前模式」写进表格的，切了会把两轮数据混在一张表里
        if self.running() || self.starting {
            self.feedback("warn", "正在运行：先「停止」再切模式（免得两个模式的结果混在一张表里）");
            return;
        }
        self.store_mode_cfg();
        // 结果数据也按模式独立：当前这份整份存回本模式槽，再取出目标模式那份
        let cur = mode_idx(self.mode);
        let dst = mode_idx(m);
        self.mode_data[cur] = self.take_mode_data();
        self.mode = m;
        self.cfg = self.cfg_modes[dst].clone();
        self.cfg.work_mode = m.key().into();
        self.restore_mode_data(dst);
        // 换模式不是配置改动：快照跟着走，别让顶栏一直提示「有改动」
        self.snapshot = self.cfg.clone();
        self.dirty_now = false;
        // 挂着的续跑询问是上一个模式的，不能带到新模式来
        self.resume_prompt = None;
    }

    /// 把当前结果数据整份取出（切模式用）
    fn take_mode_data(&mut self) -> ModeData {
        ModeData {
            items: std::mem::take(&mut self.filter_items),
            index: std::mem::take(&mut self.filter_index),
            pool: self.row_pool.take(),
            summary: std::mem::take(&mut self.summary),
            last_row_count: std::mem::replace(&mut self.last_row_count, 0),
            rows_changed: std::mem::replace(&mut self.rows_changed, false),
            ui_truncated_note: std::mem::replace(&mut self.ui_truncated_note, false),
            scan_out_dir: std::mem::take(&mut self.scan_out_dir),
        }
    }

    /// 把某模式的结果数据整份装回当前字段
    fn restore_mode_data(&mut self, i: usize) {
        let d = std::mem::take(&mut self.mode_data[i]);
        self.filter_items = d.items;
        self.filter_index = d.index;
        self.row_pool = d.pool;
        self.summary = d.summary;
        self.last_row_count = d.last_row_count;
        self.rows_changed = d.rows_changed;
        self.ui_truncated_note = d.ui_truncated_note;
        self.scan_out_dir = d.scan_out_dir;
    }

    /// 写回 findany.toml（**只有用户点「保存配置」会走到这里** —— 不再有自动保存）
    fn save_now(&mut self) -> bool {
        let path = crate::core::logfilter::autoconfig::config_path();
        self.store_mode_cfg();
        let modes = self.cfg_modes.clone();
        match crate::core::logfilter::autoconfig::save_config_all(&path, &self.cfg, &modes) {
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
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        // **和筛选走同一条管道**：同一个 spawn、同一套事件、同一个表格与按钮。
        // 唯一差异是 mode="scan" —— 管道里的 process_one 据此走「按关键字匹配」策略。
        let mut rcfg = crate::core::auto_run::filter_cfg_from_cfg(&self.cfg, &app_dir);
        rcfg.mode = "scan".into();
        // 起跑入口两个模式共用：先查有没有上次没跑完的任务（有就先问主上）
        self.begin_run_with_resume(rcfg, true);
    }

    /// **起跑前的共同入口**（扫描 / 筛选都用）：先读「上一次的任务节点」。
    ///
    /// 有未完成的节点 → 不直接开跑，挂起询问（`resume_prompt`），等主上选「继续上次 / 从头开始」。
    /// 节点由 engine 在跑的过程中每批刷新（`<out_dir>/_resume.json`），跑完自动清除。
    fn begin_run_with_resume(&mut self, mut rcfg: fengine::FilterRunCfg, scan_mode: bool) {
        let fp = crate::core::logfilter::resume::task_fingerprint(&rcfg);
        rcfg.progress_path = crate::core::logfilter::resume::progress_path(&rcfg.out_dir, &fp);
        match crate::core::logfilter::resume::load_node(&rcfg.out_dir, &rcfg.mode) {
            Some(node) => {
                self.feedback(
                    "warn",
                    format!(
                        "发现上次未完成的任务（{}，已处理 {}/{}）。请选择「继续上次」或「从头开始」",
                        crate::core::logfilter::resume::mode_label(&node.mode),
                        node.done,
                        node.total
                    ),
                );
                self.resume_prompt = Some(ResumePrompt { node, rcfg, scan_mode });
            }
            None => self.launch_run(rcfg, scan_mode, None),
        }
    }

    /// 真正启动 worker。`resume` 有值时：把上次的行载回表格 + 跳过已处理文件（断点续扫）。
    fn launch_run(
        &mut self,
        rcfg: fengine::FilterRunCfg,
        scan_mode: bool,
        resume: Option<&crate::core::logfilter::resume::ResumeNode>,
    ) {
        let root_dir = rcfg.root_dir.clone();
        // 有界通道（背压）：worker 结果堆在通道里的量封顶，满了 worker 停下等 UI。
        let (tx, rx) = std::sync::mpsc::sync_channel(CHANNEL_BOUND);
        // **按模式分派执行体**：retry 走「历史结果重传」（不遍历文件、数据来自历史 xlsx），
        // 其余两个模式走统一管道（差异只在 process_one 策略）。
        let handle = if rcfg.mode == "retry" {
            crate::core::logfilter::retry::spawn_retry(
                root_dir.clone(),
                rcfg.out_dir.clone(),
                rcfg.profile.clone(),
                rcfg.dry_run,
                tx,
            )
        } else {
            fengine::spawn_filter(rcfg, tx)
        };
        self.reset_for_new_run();
        if let Some(node) = resume {
            // 把上次已落盘的行载回表格（主上要求：续跑时要看到全量，不是只看新增）。
            // 行数据在节点指向的 JSONL 里（节点本身只有指针，避免 json 膨胀到几十 MB）。
            if !node.data_file.is_empty() {
                if let Some(p) = crate::core::logfilter::resume::load_rows(&node.data_file) {
                    self.filter_items = p.rows.clone();
                    self.rebuild_filter_index();
                }
            }
        }
        if scan_mode {
            self.scan_out_dir.clear();
            self.feedback(
                "info",
                if resume.is_some() {
                    format!("继续上次扫描：{}（已跳过已处理的文件）", root_dir)
                } else {
                    format!("正在启动扫描：{root_dir}…（后台统计文件，稍候）")
                },
            );
        } else {
            let head = if resume.is_some() {
                format!("继续上次筛选：{root_dir}（已跳过已处理的文件）")
            } else {
                format!(
                    "正在启动筛选：{}  类型「{}」  回传{}{}",
                    root_dir,
                    self.cfg.log_type,
                    if self.cfg.enabled { "开" } else { "关" },
                    if self.cfg.enabled && self.cfg.dry_run { "（dry-run）" } else { "" }
                )
            };
            self.feedback("info", head);
        }
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

    /// **一轮起跑前的共同准备**：通用扫描与日志筛选回传都调这里。
    ///
    /// 之所以抽出来：两个模式此前各写一份「清表 + 建池」，结果分叉了 ——
    /// 筛选侧接了 cache_capacity_rows、扫描侧还在用固定 5 万行上限，内存策略根本不是一套。
    /// 现在两边都走这一处：清表 + **两个池都按本轮缓存行数重建**，天然一致。
    ///
    /// 注意用新的空容器**替换**而不是 `clear()`：clear() 只清元素、保留容量，
    /// 上一轮撑到几万行时那份堆会一直挂着不还给分配器（「跑完内存降不回去」的原因之一）。
    fn reset_for_new_run(&mut self) {
        let cap = self.cfg.cache_capacity_rows.max(0) as usize;
        self.starting = true;
        self.filter_items = Vec::new();
        self.filter_index = std::collections::HashMap::new();
        self.row_pool = Some(std::sync::Arc::new(crate::core::lru_pool::RowPool::new(cap)));
        self.follow_tail = true;
    }

    /// 启动「历史结果重传」（**独立模式**）：选目录 → 递归解析 `filter_result.xlsx` → 逐文件回传。
    /// 进度节点与扫描/筛选同一套（`<out_dir>/_resume.json` 记 `done_files`），中断后可续。
    fn start_retry(&mut self) {
        if self.running() || self.starting {
            self.feedback("warn", "已在运行中，忽略重复点击");
            return;
        }
        self.cfg.resolve_out_dir(&self.proc_dir);
        let errs = self.cfg.validate();
        if !errs.is_empty() {
            let joined = errs.join("；");
            self.feedback("err", format!("无法开始重传：{joined}"));
            for e in errs {
                self.push_log("err", &e);
            }
            return;
        }
        // 重传的「根」就用上面「扫描目标」里的路径（不再另弹选目录框）：递归找里面的 filter_result.xlsx
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        let mut rcfg = crate::core::auto_run::filter_cfg_from_cfg(&self.cfg, &app_dir);
        rcfg.mode = "retry".into();
        // 重传是真调 CLI 的：找不到就别开跑（否则每个文件都「CLI 不存在」，白等一轮还全红）
        if !rcfg.dry_run && !std::path::Path::new(&rcfg.profile.cli_path).is_file() {
            let got = if rcfg.profile.cli_path.is_empty() {
                "(空)".to_string()
            } else {
                rcfg.profile.cli_path.clone()
            };
            self.feedback(
                "err",
                format!(
                    "找不到回传 CLI（{got}）：在左侧「CLI 路径」选好 intunehelper_cli.exe；留空时按程序目录 {} 找",
                    self.proc_dir.display()
                ),
            );
            return;
        }
        // 与其他两个模式**同一个起跑入口**：先读上次的进度节点决定要不要问
        self.push_log(
            "info",
            &format!(
                "重传回传 CLI：{}  类型：{}  SecretKey：{}  {}",
                rcfg.profile.cli_path,
                rcfg.upload_types.join(","),
                if self.cfg.secret_key.trim().is_empty() { "空" } else { "已填" },
                if rcfg.dry_run { "dry-run（不真调 CLI）" } else { "正式回传" }
            ),
        );
        self.begin_run_with_resume(rcfg, false);
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
        let mut rcfg = crate::core::auto_run::filter_cfg_from_cfg(&self.cfg, &app_dir);
        // 显式钉住模式：进度节点与指纹都按它取，别让配置里的旧 work_mode 把节点指到别的模式去
        rcfg.mode = "filter".into();
        // 起跑入口两个模式共用：先查有没有上次没跑完的任务（有就先问主上）
        self.begin_run_with_resume(rcfg, false);
    }

    fn stop(&mut self) {
        match &self.running {
            Running::Filter { handle, .. } => handle.cancel.store(true, Ordering::SeqCst),
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
        let n = self.filter_items.len();
        if n == 0 {
            self.feedback("warn", "表格里还没有数据，先跑一次（或等结果出现）再导出");
            return;
        }
        self.cfg.resolve_out_dir(&self.proc_dir);
        let mut cfg = self.cfg.clone();
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        // 两个模式共用同一份行数据与同一条产物写出路径（差异只在 mode）
        let rows = self.filter_items.clone();
        let fsum = self.summary.clone();
        cfg.work_mode = self.mode.key().into();
        self.spawn_job("正在导出当前数据", move || {
            let fc = crate::core::auto_run::filter_cfg_from_cfg(&cfg, &app_dir);
            // 用这次运行的真实耗时回推 t0：导出的「耗时(秒)」和跑完时是同一个数
            let t0 = Instant::now() - Duration::from_secs_f64(fsum.elapsed.max(0.0));
            match fengine::export_products(&fc, &rows, &fsum, t0) {
                Ok((dir, _excel, _audit, _kept)) => (format!("已导出 {} 条 → {dir}", rows.len()), dir),
                Err(e) => (format!("导出失败：{e}"), String::new()),
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
        self.job_started = Some(Instant::now());
        self.feedback("info", format!("{desc}…（后台进行，界面不会卡）"));
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    /// 后台任务收尾（每帧非阻塞看一眼）。
    /// 加超时兜底：目标目录在网络盘/UNC 上时 `exists()`/`create_dir_all` 可能阻塞几十秒，
    /// 旧实现会让 job_rx 一直挂着 → 「导出」「打开目录」按钮永久失效（看着像卡死）。
    /// 超过 JOB_TIMEOUT_SECS 就丢弃句柄并提示（后台线程自己会跑完，只是结果不再回填）。
    const JOB_TIMEOUT_SECS: u64 = 15;

    fn poll_job(&mut self) {
        let Some(rx) = &self.job_rx else { return };
        // 超时兜底：先看时间，再收结果
        if let Some(t0) = self.job_started {
            if t0.elapsed().as_secs() >= Self::JOB_TIMEOUT_SECS {
                self.job_rx = None;
                self.job_started = None;
                self.feedback("warn", "后台任务超时未返回（目录可能不可达），已解除占用，可重试");
                self.need_repaint = true;
                return;
            }
        }
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
                self.job_started = None;
                self.need_repaint = true;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // 还在跑：让界面继续转，状态条上的进度条会动
                self.need_repaint = true;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job_rx = None;
                self.job_started = None;
            }
        }
    }

    /// 表格截断只提示一次（避免刷屏）
    fn note_ui_truncated(&mut self) {
        if !self.ui_truncated_note {
            self.ui_truncated_note = true;
            let msg = format!(
                "表格只保留最近 {UI_ROW_CAP} 行（防内存翻倍）；明细与导出仍是全量，不受影响"
            );
            self.push_log("warn", &msg);
        }
    }

    /// 按 rel_path 就地更新：同一份日志只留一行，状态类字段直接改这一行（etest 那种状态刷新）。
    /// 走 filter_index 定位（O(1)）── 旧实现是每个行扫全表（O(N)），
    /// 一帧 2000 行 × 5000 行表 = 1000 万次字符串比较，这就是界面卡死的第二个原因。
    fn upsert_filter_row(&mut self, row: &Map<String, Value>) {
        let key = row.get("rel_path").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if let Some(&i) = self.filter_index.get(&key) {
            if let Some(slot) = self.filter_items.get_mut(i) {
                *slot = row.clone();
                return;
            }
            // 索引指向的位置已失效（理论上不该发生：淘汰/插头都会 rebuild）── 兜底重建
            self.rebuild_filter_index();
        }
        self.filter_index.insert(key, self.filter_items.len());
        self.filter_items.push(row.clone());
    }

    /// 淘汰（尾部截断 / 头部移除）或头部插入后下标整体偏移 ── 必须重建索引。
    /// 批量淘汰只调一次，O(N)；比每行扫全表便宜一个数量级。
    fn rebuild_filter_index(&mut self) {
        self.filter_index.clear();
        self.filter_index.reserve(self.filter_items.len());
        for (i, r) in self.filter_items.iter().enumerate() {
            if let Some(k) = r.get("rel_path").and_then(|v| v.as_str()) {
                self.filter_index.insert(k.to_string(), i);
            }
        }
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

    /// 从历史结果重传：**选一个目录**（输出根或某个批次目录），**递归**找里面所有
    /// `filter_result.xlsx`，逐个只重传「上次失败 / 未回传 / dry-run」的台，
    /// 结果写回各自的原表（写前自动备份）。回传参数用**当前界面配置**（xlsx 不存凭据）。
    ///
    /// 为什么是目录：输出根下是一堆批次目录，主上选根就能一次把历次批次全重传一遍；
    /// 目录递归在**后台线程**做（网络盘上遍历可能很慢，压在 UI 线程就是卡死）。
    fn retry_from_history(&mut self) {
        if self.job_rx.is_some() {
            self.feedback("warn", "上一个后台任务（导出/打开目录/重传）还没结束，稍等再点");
            return;
        }
        if self.running() || self.starting {
            self.feedback("warn", "正在跑任务，等结束再重传（避免同时写同一批产物）");
            return;
        }
        let Some(dir) = pick_folder() else { return };
        let dir_s = dir.to_string_lossy().to_string();
        let app_dir = self.proc_dir.to_string_lossy().to_string();
        let rcfg = crate::core::auto_run::filter_cfg_from_cfg(&self.cfg, &app_dir);
        let profile = rcfg.profile.clone();
        let dry_run = self.cfg.dry_run;
        self.spawn_job("正在从历史结果重传（递归扫描目录）", move || {
            // 递归展开目录：找 filter_result.xlsx
            let files = collect_filter_xlsx(std::path::Path::new(&dir_s));
            if files.is_empty() {
                return (
                    format!("{dir_s} 下没找到 filter_result.xlsx（要选 findany 的输出根，或某个批次目录）"),
                    String::new(),
                );
            }
            match run_retry_job(&files, &profile, dry_run) {
                Ok(msg) => (msg, String::new()),
                Err(e) => (format!("重传失败：{e}"), String::new()),
            }
        });
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
        // 单帧事件预算：数据量大时通道里可能堆着几万条事件，一帧全消费会把界面卡死；
        // 超预算就留到下一帧（try_recv 队列不会丢事件）。
        let budget = Instant::now() + Duration::from_millis(6);
        // **按「行数」而非「事件数」限额**：一个 Batch 事件最多含 64 行、每行含 4000 字符
        // HardwareHash（真样本整行 ~13KB）。旧上限 2000 事件 = 最多 12.8 万行 ≈ 1.6GB 一次性
        // 拉进内存 —— 这就是「界面卡死 + 内存从 170MB 冲到 1400MB」的直接原因之一。
        // 2000 行 × 13KB ≈ 26MB：安全，且一帧足以消化。
        const MAX_ROWS_PER_FRAME: usize = 2000;
        let mut rows_in_frame = 0usize;

        // 事件轮询：**两个模式共用这一条分支**（扫描/筛选是同一条管道，事件类型也同一个）
        let filter_shot = if let Running::Filter { rx, .. } = &mut self.running {
            let mut evs: Vec<FilterEvent> = Vec::new();
            loop {
                if rows_in_frame >= MAX_ROWS_PER_FRAME || Instant::now() > budget {
                    self.events_backlogged = true;
                    break;
                }
                match rx.try_recv() {
                    Ok(ev) => {
                        if let FilterEvent::Batch(b) = &ev {
                            rows_in_frame += b.items.len();
                        }
                        evs.push(ev);
                    }
                    Err(_) => break,
                }
            }
            let mut done = None;
            let mut evicted_any = false;
            for ev in evs {
                match ev {
                    FilterEvent::Log(lvl, msg) => self.push_log(&lvl, &msg),
                    FilterEvent::Progress { .. } => {}
                    FilterEvent::Batch(batch) => {
                        // 逐行 upsert（走 filter_index，O(1)/行）── 同一行只留一份，
                        // 状态刷新（回传改状态）就地把那一行换掉，不新增行。
                        for it in batch.items {
                            self.upsert_filter_row(&it);
                        }
                        // 「启动中」的结束时机：首批结果到达就切到运行态。
                        // 旧实现只在 Done 时才清 starting ── filter 模式整个运行期都显示
                        // 「启动中…（点此取消）」，用户看不到「进行中」也不知道能不能停。
                        if self.starting {
                            self.starting = false;
                            self.feedback("info", format!("筛选进行中：已出 {} 行", self.filter_items.len()));
                        }
                        // UI 端 LRU：超 cache_capacity_rows 把最旧的移到 row_pool
                        // （表格底部「加载更多」可拉回）。批量 drain 后重建索引（下标整体偏移）。
                        if let Some(pool) = self.row_pool.clone() {
                            let cap = self.cfg.cache_capacity_rows.max(0) as usize;
                            if cap > 0 && self.filter_items.len() > cap {
                                let excess = self.filter_items.len() - cap;
                                let moved: Vec<Map<String, Value>> = self.filter_items.drain(0..excess).collect();
                                for it in moved {
                                    pool.push(it);
                                }
                                evicted_any = true;
                            }
                        }
                        if trim_rows(&mut self.filter_items) {
                            self.note_ui_truncated();
                            evicted_any = true;
                        }
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
            if evicted_any {
                self.rebuild_filter_index();
            }
            done
        } else {
            None
        };

        // 收尾：**两个模式共用这一条**（扫描 / 筛选都是这条管道），结论按 self.mode 分派
        if let Some((outcome, note)) = filter_shot {
            // 就地归并（upsert 走 filter_index，O(1)/行；收尾这几万行也不会再卡）
            for r in &outcome.items {
                self.upsert_filter_row(r);
            }
            // 收尾后按缓存上限淘汰（最旧的移到 row_pool，索引随后重建）
            let mut evicted_tail = false;
            if let Some(pool) = self.row_pool.clone() {
                let cap = self.cfg.cache_capacity_rows.max(0) as usize;
                if cap > 0 && self.filter_items.len() > cap {
                    let excess = self.filter_items.len() - cap;
                    let moved: Vec<Map<String, Value>> = self.filter_items.drain(0..excess).collect();
                    for it in moved {
                        pool.push(it);
                    }
                    evicted_tail = true;
                }
            }
            if trim_rows(&mut self.filter_items) {
                self.note_ui_truncated();
                evicted_tail = true;
            }
            if evicted_tail {
                self.rebuild_filter_index();
            }
            self.summary = outcome.summary.clone();
            self.starting = false;
            // 先取出需要的标量/字符串（避免后面 &self.summary 与 &mut self 冲突）
            let (s_hit, s_miss, s_skipped, s_total, s_elapsed, s_batch, s_excel) = {
                let s = &self.summary;
                (s.hit, s.miss, s.skipped, s.total, s.elapsed, s.batch_dir.clone(), s.excel_path.clone())
            };
            let target = if s_batch.is_empty() { String::new() } else { format!("{s_batch}  ->  {s_excel}") };
            if self.mode == WorkMode::Scan {
                // —— 通用扫描结论（与筛选同一套收尾，只是文案/判定不同）——
                self.push_log("ok", &format!("完成：命中 {s_hit} / 未命中 {s_miss} / 跳过 {s_skipped}，耗时 {s_elapsed}s"));
                self.push_log("info", &format!("输出：{target}"));
                self.scan_out_dir = s_batch.clone();
                let ok = s_hit > 0;
                let content = format!(
                    "扫描 命中 {}/{}（未命中 {}，跳过 {}），耗时 {}s，产物 {}",
                    s_hit, s_total, s_miss, s_skipped, s_elapsed, target
                );
                crate::core::result::emit(&content, ok, "scan");
                let mark = if ok { "PASS" } else { "FAIL" };
                self.feedback(if ok { "ok" } else { "warn" }, format!("扫描完成（{mark}）：{content}"));
                // 与筛选模式一致：允许走 auto_close 倒计时（旧实现传 false → 扫描永不倒计时）
                self.finish_ui(true);
                let _ = note;
                return;
            }
            {
                // 收尾也走就地归并：表里已经是这些行，只更新字段（不再整表替换 -> 不会出现「最后清空重建」）
                // 逐行 upsert 走 filter_index，收尾这几万行也不会再卡（旧的 O(N) 扫描在这里最痛）
                for r in &outcome.items {
                    self.upsert_filter_row(r);
                }
                // 收尾后同样按缓存上限淘汰（把最旧的移到 row_pool，索引随后重建）
                let mut evicted_tail = false;
                if let Some(pool) = self.row_pool.clone() {
                    let cap = self.cfg.cache_capacity_rows.max(0) as usize;
                    if cap > 0 && self.filter_items.len() > cap {
                        let excess = self.filter_items.len() - cap;
                        let moved: Vec<Map<String, Value>> = self.filter_items.drain(0..excess).collect();
                        for it in moved {
                            pool.push(it);
                        }
                        evicted_tail = true;
                    }
                }
                if trim_rows(&mut self.filter_items) {
                    self.note_ui_truncated();
                    evicted_tail = true;
                }
                if evicted_tail {
                    self.rebuild_filter_index();
                }
                self.summary = outcome.summary.clone();
                let line = {
                    let s = &self.summary;
                    if self.mode == WorkMode::Retry {
                        format!(
                            "重传完成：文件 {}/{}，回传 成功 {} / 冲突 {} / 失败 {}{}",
                            s.extracted, s.total, s.upload_ok, s.upload_conflict, s.upload_fail,
                            if s.upload_dry > 0 { format!(" / dry-run {}", s.upload_dry) } else { String::new() }
                        )
                    } else {
                        format!(
                            "筛选完成：提取 {}/{}（未知类型 {}），回传 成功 {} / 冲突 {} / 失败 {}{}",
                            s.extracted, s.total, s.unknown, s.upload_ok, s.upload_conflict, s.upload_fail,
                            if s.upload_dry > 0 { format!(" / dry-run {}", s.upload_dry) } else { String::new() }
                        )
                    }
                };
                self.push_log("ok", &line);
                // 重传是「必然回传」：判定只看实际跑了多少（整目录都无需重传 = 正常，不算异常）
                let upload_on = if self.mode == WorkMode::Retry {
                    self.summary.upload_ok + self.summary.upload_conflict + self.summary.upload_fail > 0
                } else {
                    self.cfg.enabled
                };
                // 拦截（都不判 PASS、不倒计时不关，自动化跑歪了必须有人看到）：
                //  ① 数据为空；② 开了正式回传却没真的回传（定向类型一台没匹配到 / 一台都没回传 / 有失败冲突）
                // ② 的判定与 R 结论共用 upload_anomaly，界面与验收输出不会各说一套。
                let empty = self.summary.total == 0 || self.summary.extracted == 0;
                if empty {
                    let what = if self.mode == WorkMode::Retry { "filter_result.xlsx" } else { "可处理日志" };
                    // 剩余为 0（续跑时全被跳过）= 上次其实已经跑到末尾了：清掉本模式的节点，
                    // 免得下次点开始又被问一遍「继续上次」，问完还是什么都不跑。
                    crate::core::logfilter::resume::clear_node(&self.cfg.out_dir, self.mode.key());
                    self.push_log(
                        "err",
                        &format!("数据为空拦截：本次没有需要处理的{what}（文件 {}，提取 {}），程序保持打开", self.summary.total, self.summary.extracted),
                    );
                    self.finish_ui(false);
                } else if let Some(why) = fengine::upload_anomaly(&self.summary, upload_on, self.cfg.dry_run) {
                    self.push_log("err", &format!("回传拦截：{why}；程序保持打开待处理"));
                    self.finish_ui(false);
                } else {
                    let _ = note;
                    self.finish_ui(true);
                }
                // 验收输出：R<{content,status,opts}>R（与 --auto 同一套结论）
                self.starting = false;
                // 结论：重传与日志筛选各用各的口径（同一份 summary，界面与 --auto 判的是同一套）
                let (content, ok) = if self.mode == WorkMode::Retry {
                    crate::core::logfilter::retry::retry_verdict(&self.summary, self.cfg.dry_run)
                } else {
                    fengine::filter_verdict(&self.summary, upload_on, self.cfg.dry_run)
                };
                crate::core::result::emit(&content, ok, "filter");
                let mark = if ok { "PASS" } else { "FAIL" };
                self.feedback(if ok { "ok" } else { "err" }, format!("筛选完成（{mark}）：{content}"));
            }
            let _ = note;
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
        // **两个模式同一套完成弹窗规则**（主上要求同步）：
        //   auto_close 开 + 允许倒计时 + 有数据 + （筛选侧）回传没有异常
        //   → 倒计时并自动关；否则只弹提示不倒计时。
        // 旧实现按 mode 分支：筛选受 auto_close 控制、扫描无条件 Some(0) —— 同样的操作
        // 在两个模式下的弹窗/倒计时行为不一样。
        let upload_bad = self.mode == WorkMode::Filter && upload_on && ran && !all_ok;
        if empty {
            // 本次一个结果都没有（续跑时剩余已被跳完 / 目录里没有可处理的文件）：
            // **不弹「完成」窗口** —— 否则看起来就是「明明没运行却提示完成」。
            // 原因已经写在日志与状态条里（数据为空拦截那一条）。
            self.countdown = None;
            return;
        }
        if self.cfg.auto_close && allow_countdown && !upload_bad {
            self.countdown = Some(self.cfg.countdown_sec.max(1) as u32);
        } else {
            self.countdown = Some(0); // 只弹提示，不自动关
        }
    }

    fn tick_countdown(&mut self) {
        let Some(sec) = self.countdown else { return };
        // 与 finish_ui 同口径：不再限制只有筛选模式才倒计时
        if sec == 0 || !self.cfg.auto_close {
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
            // 工作模式：三档（通用扫描 / 日志筛选回传 / 历史结果重传）
            egui::ComboBox::from_id_salt("work_mode")
                .width(290.0)
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for m in [WorkMode::Scan, WorkMode::Filter, WorkMode::Retry] {
                        if ui.selectable_label(self.mode == m, m.label()).clicked() && self.mode != m {
                            self.switch_mode(m);
                        }
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
            // 「停止」= 红色（主上指定）：危险/终止动作要一眼看到，区别于中性的常规按钮。
            let btn = egui::Button::new(RichText::new("停止").size(SIZE_BODY).strong().color(p.err))
                .fill(p.err.gamma_multiply(0.15))
                .stroke(egui::Stroke::new(1.0, p.err))
                .min_size(size);
            if ui.add(btn).clicked() {
                self.stop();
            }
            return;
        }
        let btn = egui::Button::new(RichText::new("开始").size(SIZE_BODY).strong().color(Color32::WHITE))
            .fill(p.brand)
            .min_size(size);
        if ui.add(btn).clicked() {
            // 三档模式各自的启动入口
            match self.mode {
                WorkMode::Scan => self.start_scan(),
                WorkMode::Filter => self.start_filter(),
                WorkMode::Retry => self.start_retry(),
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
            // 搜索框：按文件名子串筛（不分大小写），留空=不过滤。和「文件类型」是「与」关系
            let mut enter_start = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(SIZE_BODY).color(p.text2));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.cfg.name_filter)
                        .desired_width(f32::INFINITY)
                        .hint_text("搜索文件名包含（留空=不过滤，回车=开始）"),
                );
                resp.clone().on_hover_text("只处理文件名里含这段文字的文件，不分大小写；和下面的「文件类型」同时生效；按回车直接开始");
                // 回车 = 开始（和顶栏那颗按钮同一条路）
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    enter_start = true;
                }
            });
            if enter_start {
                if self.mode == WorkMode::Scan {
                    self.start_scan();
                } else {
                    self.start_filter();
                }
            }
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
                    // 重传找的是历史结果文件，不提「扩展名过滤」（那套只对日志有意义）
                    PathKind::File => (
                        if self.mode == WorkMode::Retry {
                            "识别：单个结果文件（只处理这一个）".to_string()
                        } else {
                            "识别：单个文件（只处理这一个，不走扩展名过滤）".to_string()
                        },
                        p.ok,
                    ),
                    PathKind::Dir => {
                        let scope = if self.cfg.recursive { "含子目录" } else { "仅本层" };
                        let what = if self.mode == WorkMode::Retry { "找 filter_result.xlsx" } else { "按扩展名过滤" };
                        (format!("识别：目录（{scope}，{what}）"), p.ok)
                    }
                    PathKind::Checking => ("识别中…（网络盘可能要等一下）".to_string(), p.text2),
                    PathKind::Missing => (format!("识别：路径不存在 —— {t}"), p.err),
                }
            };
            ui.label(RichText::new(txt).size(SIZE_SMALL).color(col));
            ui.add_space(2.0);
            // ---- 文件类型过滤（只扫 log / txt 这类）----
            // 点标签就切换进 cfg.extensions（唯一的真源，写回 findany.toml）；「全部」= 空列表 = 不筛
            if self.mode == WorkMode::Retry {
                // 重传不按扩展名扫目录，只认历史结果文件 —— 类型就固定成 xlsx（选择仍在「扫描目标」这一处）
                ui.horizontal_wrapped(|ui| {
                    row_label(ui, p, "文件类型");
                    let _ = ui.selectable_label(true, RichText::new("xlsx").size(SIZE_BODY));
                });
                ui.label(theme::dim("当前：只处理 filter_result.xlsx（xlsx）", p));
            } else {
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
            }
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
            // 重传：回传口径只有 etest(OA3)、历史结果文件只有 xlsx，所以这两项各自都只有一个选项；
            // 「启用数据回传」开关对它没意义（重传本身就是要传），也不显示。
            let is_retry = self.mode == WorkMode::Retry;
            let gtitle = if is_retry { "历史结果重传" } else { "日志筛选 / 回传" };
            let ghint = if is_retry { "filter_result.xlsx" } else { "etest 系" };
            group(ui, p, gtitle, ghint, |ui| {
                if is_retry {
                    ui.horizontal(|ui| {
                        row_label(ui, p, "回传类型");
                        let cur = self.cfg.types.first().cloned().unwrap_or_else(|| "etest(OA3)".to_string());
                        egui::ComboBox::from_id_salt("retry_upload_type")
                            .width(180.0)
                            .selected_text(cur.as_str())
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(cur == "etest(OA3)", "etest(OA3)").clicked() {
                                    self.cfg.types = vec!["etest(OA3)".to_string()];
                                }
                            });
                    });
                } else {
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
                }
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
                row_label(ui, p, "缓存行数");
                let mut rows = self.cfg.cache_capacity_rows;
                let dv = egui::DragValue::new(&mut rows)
                    .range(0..=100_000)
                    .custom_formatter(|v, _| if v == 0.0 { "不限".to_string() } else { format!("{v:.0} 行") });
                if ui.add(dv).changed() {
                    self.cfg.cache_capacity_rows = rows;
                }
            })
            .response
            .on_hover_text(
                "文件数上限：最多处理多少个文件，0=不限（防目录跑飞的保险丝）。\n\
                 缓存行数：内存里最多留多少行结果，超出立即淘汰最旧（被淘汰的行可从表格底部「加载更多」拉回）；0=不限。\n\
                 这一项直接决定内存占用 —— 十万份日志按 5000 行缓存的常驻内存约 60MB，不限则会到 GB 级。",
            );
            // 内存上限：百万级目录最怕被系统挤爆（那种死法没有 panic、没有弹窗、日志停在半截）
            ui.horizontal_wrapped(|ui| {
                row_label(ui, p, "内存上限");
                let mut mem = self.cfg.mem_limit_mb;
                let dv = egui::DragValue::new(&mut mem)
                    .range(0..=1_000_000)
                    .suffix(" MB")
                    .custom_formatter(|v, _| if v == 0.0 { "自动(物理内存90%)".to_string() } else { format!("{v:.0} MB") });
                if ui.add(dv).changed() {
                    self.cfg.mem_limit_mb = mem;
                }
                ui.label(theme::dim("到上限主动安全停止（不会被系统挤爆）", p));
            });
            // 分批模式：root_dir 下每个一级子目录各跑一批（百万级目录推荐：内存只跟最大子目录有关）
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.cfg.batch_dirs, RichText::new("按子目录分批").size(SIZE_BODY));
                ui.label(theme::dim("批次名含", p));
                let mut bf = self.cfg.batch_name_filter.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut bf).desired_width(120.0).hint_text("空=全部"))
                    .changed()
                {
                    self.cfg.batch_name_filter = bf;
                }
            })
            .response
            .on_hover_text("勾上：root_dir 下每个一级子目录各跑一批（整轮一条 R 结论，带批次汇总）；右框只挑名字含该段的子目录");
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
        // **两个模式共用这一条**：同一个句柄、同一份 progress
        let (live_done, live_total, live_pct, live_extracted, live_unknown, live_hit, live_miss, live_skip, running_secs) =
            match &self.running {
                Running::Filter { t0, handle, .. } => {
                    let p = handle.progress();
                    (
                        p.done,
                        p.total,
                        p.pct,
                        p.extracted,
                        p.unknown,
                        p.hit,
                        p.miss,
                        p.skipped,
                        t0.elapsed().as_secs_f64(),
                    )
                }
                Running::Idle => (0, 0, 0.0, 0, 0, 0, 0, 0, 0.0),
            };
        // 重传是「每个 xlsx 一行」，计数口径与筛选一致（回传成功 / 失败），跟扫描的命中 / 未命中不搭边
        let is_retry = self.mode == WorkMode::Retry;
        let is_filter = matches!(self.mode, WorkMode::Filter | WorkMode::Retry);
        let running = self.running();
        // 运行中读实时计数；结束后读 summary（三个模式的计数都在同一个 FilterSummary 里）
        let (a, b, c) = if running {
            if is_filter {
                (live_extracted, live_unknown, live_skip)
            } else {
                (live_hit, live_miss, live_skip)
            }
        } else if is_filter {
            (self.summary.extracted, self.summary.unknown, self.summary.skipped)
        } else {
            (self.summary.hit, self.summary.miss, self.summary.skipped)
        };
        let elapsed = self.summary.elapsed;
        // 回传成功 / 冲突 / 失败：运行中直接从 handle 的 Atomic 读（跟手），结束后读 summary
        let (up_ok, up_conflict, up_fail) = match &self.running {
            Running::Filter { handle, .. } => (
                handle.upload_ok.load(Ordering::Relaxed),
                handle.upload_conflict.load(Ordering::Relaxed),
                handle.upload_fail.load(Ordering::Relaxed),
            ),
            _ => (self.summary.upload_ok, self.summary.upload_conflict, self.summary.upload_fail),
        };

        ui.horizontal(|ui| {
            if is_retry {
                // 重传没有「命中」这回事，看的就是回传结果本身
                stat(ui, p, "回传成功", &up_ok.to_string(), p.ok);
                stat(ui, p, "回传失败", &up_fail.to_string(), if up_fail > 0 { p.err } else { p.text2 });
                stat(ui, p, "冲突(人工)", &up_conflict.to_string(), p.warn);
            } else {
                stat(ui, p, if is_filter { "提取成功" } else { "命中" }, &a.to_string(), p.ok);
                stat(ui, p, if is_filter { "未知类型" } else { "未命中" }, &b.to_string(), p.text2);
                stat(ui, p, "跳过", &c.to_string(), p.warn);
            }
            let total_files = if running { live_total } else { self.summary.total };
            // 遍历阶段总数还是 0：显式提示「正在统计」，避免看着像卡住
            let total_txt = if running && total_files == 0 { "统计中…".to_string() } else { total_files.to_string() };
            stat(ui, p, "文件总数", &total_txt, p.text);
            // 目录统计（统计阶段落到索引里；completed 后下次复用、不再重新遍历）
            let (idx_dirs, idx_subs, idx_ready) = match &self.running {
                Running::Filter { handle, .. } => (
                    handle.idx_dirs.load(Ordering::Relaxed),
                    handle.idx_sub_dirs.load(Ordering::Relaxed),
                    handle.idx_ready.load(Ordering::Relaxed),
                ),
                Running::Idle => (self.summary.idx_dirs, self.summary.idx_sub_dirs, self.summary.idx_dirs > 0),
            };
            if idx_dirs > 0 || idx_subs > 0 {
                stat(
                    ui,
                    p,
                    if idx_ready { "目录（已统计）" } else { "目录（统计中）" },
                    &format!("{}（子目录 {}）", idx_dirs, idx_subs),
                    if idx_ready { p.text } else { p.warn },
                );
            }
            // 回传成功 / 失败：只在日志筛选模式重复显示（扫描没有回传概念；重传上面已经显示过）
            if self.mode == WorkMode::Filter {
                stat(ui, p, "回传成功", &up_ok.to_string(), p.ok);
                stat(ui, p, "回传失败", &up_fail.to_string(), if up_fail > 0 { p.err } else { p.text2 });
            }
            stat(ui, p, if running { "已用时" } else { "耗时" }, &format!("{:.2}s", if running { running_secs } else { elapsed }), p.info);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Running::Filter { handle, up_total, up_index, up_pct, sn, status, .. } = &self.running {
                    // 实时累计：直接读 worker 的 AtomicUsize（每帧都是最新值）──
                    // 不必等 Done 事件就能看到「成功 / 冲突 / 失败 / dry-run」当场在变。
                    let ok = handle.upload_ok.load(Ordering::Relaxed);
                    let conflict = handle.upload_conflict.load(Ordering::Relaxed);
                    let fail = handle.upload_fail.load(Ordering::Relaxed);
                    let dry = handle.upload_dry.load(Ordering::Relaxed);
                    let ran = ok + conflict + fail + dry;
                    if *up_total > 0 {
                        ui.label(theme::dim(format!("回传 {}/{}  {}  {}", up_index, up_total, sn, status), p));
                        ui.add(egui::ProgressBar::new((*up_pct as f32 / 100.0).clamp(0.0, 1.0)).desired_width(140.0));
                    }
                    if ran > 0 {
                        ui.label(theme::dim(
                            format!(
                                "已回传 {}：✓{} ⚠{} ✗{}{}",
                                ran, ok, conflict, fail,
                                if dry > 0 { format!(" / dry-run {}", dry) } else { String::new() }
                            ),
                            p,
                        ));
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
                let n = self.filter_items.len();
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
                // 历史结果重传：**选目录 + 递归**（输出根→历次批次一次全跑）。
                // 只重传「回传状态为空/失败/dry-run」的行，结果写回各自原 xlsx（写前自动备份）。
                if self.mode == WorkMode::Filter
                    && ui
                        .add(egui::Button::new(theme::body("从历史结果重传", p)).min_size(egui::vec2(140.0, BTN_H)))
                        .on_hover_text(
                            "选一个目录（输出根 / 某个批次目录），递归找里面所有 filter_result.xlsx：\n\
                             只重传「回传状态为空 / 失败 / dry-run」的行，成功与「冲突(人工)」跳过。\n\
                             回传参数用当前界面配置；结果写回各自原文件（写前自动备份 .bak-时间戳）。",
                        )
                        .clicked()
                {
                    self.retry_from_history();
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
            if self.mode == WorkMode::Retry {
                // 独立模式：结果是「每个 xlsx 一行」
                retry_table(ui, p, &self.filter_items, self.follow_tail, "retry_table", changed);
            } else {
                filter_table(ui, p, &self.filter_items, self.follow_tail, "filter_table", changed);
            }
        } else {
            // 扫描与筛选**同一条管道、同一份行数据**（Vec<Map>），只是这里的列定义不同
            let changed = self.rows_changed;
            scan_table(ui, p, &self.filter_items, self.follow_tail, "scan_table", changed);
        }
        // 表格底部「加载更多」：两个模式**共用同一个池与同一段渲染** ──
        // 池里有被缓存上限淘汰的旧行时露出按钮，点了拉回插到表头（下标整体偏移，须重建索引）。
        if let Some(pool) = self.row_pool.clone() {
            let pool_len = pool.pool_len();
            if pool_len > 0 {
                ui.horizontal(|ui| {
                    let step = pool_len.min(self.cfg.cache_capacity_rows.max(1) as usize);
                    let label = load_more_label(pool_len, step);
                    if ui
                        .add(egui::Button::new(theme::body(&label, p)).min_size(egui::vec2(220.0, BTN_H)))
                        .clicked()
                    {
                        let moved = pool.take_more(step);
                        if !moved.is_empty() {
                            self.filter_items.splice(0..0, moved);
                            self.rebuild_filter_index();
                            self.rows_changed = true;
                            self.need_repaint = true;
                        }
                    }
                });
            }
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

    /// 续跑询问窗口：点开始时发现上次没跑完 → 让主上定「继续上次」还是「从头开始」。
    /// 每次问（主上定的），不自动续也不自动清。
    fn resume_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(prompt) = self.resume_prompt.take() else { return };
        let mut decided: Option<bool> = None; // Some(true)=继续 Some(false)=从头
        egui::Window::new("发现未完成的任务")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("上次的任务没跑完").size(SIZE_HEAD).strong());
                ui.label(
                    RichText::new(format!(
                        "{}：已处理 {}/{}",
                        crate::core::logfilter::resume::mode_label(&prompt.node.mode),
                        prompt.node.done,
                        prompt.node.total
                    ))
                    .size(SIZE_SMALL)
                    .color(p.text2),
                );
                if !prompt.node.root_dir.is_empty() {
                    ui.label(theme::dim(format!("目录：{}", prompt.node.root_dir), p));
                }
                if !prompt.node.updated_at.is_empty() {
                    ui.label(theme::dim(format!("最后更新：{}", prompt.node.updated_at), p));
                }
                if !prompt.node.done_dirs.is_empty() {
                    ui.label(theme::dim(
                        format!(
                            "已跑完 {} 个文件夹（续跑时整份跳过；没跑完的那个会重跑一遍）",
                            prompt.node.done_dirs.len()
                        ),
                        p,
                    ));
                }
                ui.label(theme::dim("继续：把上次的行载回表格，只跑剩下的文件夹（已跑完的跳过）", p));
                ui.label(theme::dim("从头开始：丢掉旧进度，所有文件重新跑一遍", p));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(theme::body("继续上次", p)).min_size(egui::vec2(120.0, BTN_H)))
                        .clicked()
                    {
                        decided = Some(true);
                    }
                    if ui
                        .add(egui::Button::new(theme::body("从头开始", p)).min_size(egui::vec2(120.0, BTN_H)))
                        .clicked()
                    {
                        decided = Some(false);
                    }
                });
            });
        match decided {
            Some(true) => {
                // 续跑：行数据在节点指向的 JSONL 里（节点只有指针），跳过已处理文件
                let mut rcfg = prompt.rcfg.clone();
                // 跳过粒度 = **文件夹**：上次跑完的文件夹整份跳过（没跑完的那个会重跑一遍）
                rcfg.skip_dirs = prompt.node.done_dirs.iter().cloned().collect();
                self.push_log(
                    "info",
                    &format!(
                        "继续上次任务（{}）：已处理 {}/{}，跳过已完成的文件",
                        crate::core::logfilter::resume::mode_label(&prompt.node.mode),
                        prompt.node.done,
                        prompt.node.total
                    ),
                );
                let node = prompt.node.clone();
                self.launch_run(rcfg, prompt.scan_mode, Some(&node));
            }
            Some(false) => {
                // 从头：节点与数据文件都清掉；目录统计（索引）也清掉，下次重新统计
                crate::core::logfilter::resume::clear_node(&prompt.rcfg.out_dir, &prompt.rcfg.mode);
                let fp = crate::core::logfilter::resume::task_fingerprint(&prompt.rcfg);
                crate::core::logfilter::treeindex::clear_index(&prompt.rcfg.out_dir, &fp);
                if !prompt.node.data_file.is_empty() {
                    let _ = std::fs::remove_file(&prompt.node.data_file);
                }
                self.push_log("warn", "已丢弃上次的进度，从头开始跑");
                self.launch_run(prompt.rcfg, prompt.scan_mode, None);
            }
            None => {
                // 还没点：放回去，下一帧继续显示
                self.resume_prompt = Some(prompt);
            }
        }
    }

    fn countdown_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(sec) = self.countdown else { return };
        let mut close_now = false;
        let is_retry = self.mode == WorkMode::Retry;
        let is_filter = matches!(self.mode, WorkMode::Filter | WorkMode::Retry);
        egui::Window::new("完成")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let title = if is_retry { "历史结果重传完成" } else if is_filter { "日志筛选完成" } else { "扫描完成" };
                let sub = if is_retry {
                    format!(
                        "回传 成功 {} / 冲突 {} / 失败 {}",
                        self.summary.upload_ok, self.summary.upload_conflict, self.summary.upload_fail
                    )
                } else if is_filter {
                    format!("产物：{}", self.summary.batch_dir)
                } else {
                    format!("命中 {}，未命中 {}，跳过 {}", self.summary.hit, self.summary.miss, self.summary.skipped)
                };
                ui.label(RichText::new(title).size(SIZE_HEAD).strong());
                ui.label(RichText::new(sub).size(SIZE_SMALL).color(p.text2));
                // 倒计时提示两个模式都显示（与 finish_ui 同口径）
                if self.cfg.auto_close && sec > 0 {
                    ui.label(RichText::new(format!("{sec} 秒后自动关闭程序")).size(SIZE_SMALL).color(p.warn));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(theme::body("打开输出目录", p)).min_size(egui::vec2(130.0, BTN_H))).clicked() {
                        self.open_out_dir();
                    }
                    // 「延时」只在真的在倒计时时才给（auto_close 关着的时候点了也不会动）
                    if self.cfg.auto_close && sec > 0
                        && ui.add(egui::Button::new(theme::body("延时 30s", p)).min_size(egui::vec2(100.0, BTN_H))).clicked()
                    {
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
        // 倒计时在走：保持 500ms 重绘（两个模式一致）
        if self.cfg.auto_close && sec > 0 {
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
    // 造 3 行测试数据（统一管道的行结构 = Map）
    app.filter_items = (0..3)
        .map(|i| {
            let mut m = Map::new();
            m.insert("rel_path".into(), Value::String(format!("f{i}.log")));
            m.insert("filename".into(), Value::String(format!("f{i}.log")));
            m.insert("dir_name".into(), Value::String(".".into()));
            m.insert("hit".into(), Value::Bool(true));
            m.insert("hit_line_text".into(), Value::String("x".into()));
            m.insert("size_str".into(), Value::String("1.0 KB".into()));
            m.insert("mtime_str".into(), Value::String("2026-09-22 00:00:00".into()));
            m.insert("encoding".into(), Value::String("utf-8".into()));
            m.insert("extract_ok".into(), Value::Bool(true));
            m
        })
        .collect();
    app.rebuild_filter_index();
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

/// 表格保留的最近行数：界面那一份复制要有界。
/// 产物/导出不受影响 —— 明细是引擎侧自己写的（scan/export 在交界面之前就落盘了），
/// 这里只裁「显示用的那一份」，几十万行也不会再把内存翻倍。
pub const UI_ROW_CAP: usize = 50_000;

/// worker -> UI 事件通道的**有界**容量（背压）。
/// 无界通道下 worker 跑得比 UI 渲染快时，结果会在通道里堆到几十万行
/// （每行含 4000 字符 HardwareHash ≈ 13KB，十万行 ≈ 1.3GB）—— 这是运行中内存
/// 峰值远超实际数据量的主因。有界后队列最多这么多个批次，满了 worker 就阻塞等 UI。
/// 32 个批次 × 每批最多 64 行 ≈ 2048 行 ≈ 27MB 封顶。
pub const CHANNEL_BOUND: usize = 32;

/// 超过上限就丢掉最旧的（返回 true=发生了截断）
fn trim_rows<T>(rows: &mut Vec<T>) -> bool {
    if rows.len() > UI_ROW_CAP {
        let cut = rows.len() - UI_ROW_CAP;
        rows.drain(0..cut);
        true
    } else {
        false
    }
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
/// 内部把测试用 ScanItem 转成管道的 Row(Map)，走的正是运行时那条渲染路径。
pub fn render_scan_table_for_test(ui: &mut egui::Ui, items: &[ScanItem], follow: bool) -> (f32, f32, f32, f32) {
    // 一帧只画一张表：表格在 Ui 里是竖着排的，画两张时第二张会被挤出可视区，
    // 布局会被 egui 裁剪掉，量到的几何就不是真实值了。
    // follow=true 走「本帧有新行 → 钉到底」那条分支（历史上正是在这里 NaN 崩溃）。
    let p = theme::dark();
    let salt = if follow { "uitest_table_follow" } else { "uitest_table" };
    let rows: Vec<Map<String, Value>> = items.iter().map(scan_item_to_row).collect();
    scan_table(ui, &p, &rows, follow, salt, true)
}

/// 回归自证入口（--uitest 用）：画一遍**历史结果重传**的结果表。
/// 重传以前只是筛选模式里的一个按钮（走的是筛选表），独立成模式后这张表第一次真正上屏，
/// 这里连同首帧 / 0 尺寸 / NaN 布局一起锁住。
pub fn render_retry_table_for_test(ui: &mut egui::Ui, follow: bool) -> (f32, f32, f32, f32) {
    let p = theme::dark();
    let mk = |file: &str, target: i64, ok: i64, fail: i64, state: &str| {
        let mut m = Map::new();
        m.insert("file_name".into(), Value::String(file.into()));
        m.insert("dir".into(), Value::String("out/2026-09-23_10-00".into()));
        m.insert("rows_total".into(), Value::Number(12.into()));
        m.insert("retry_targets".into(), Value::Number(target.into()));
        m.insert("ok".into(), Value::Number(ok.into()));
        m.insert("conflict".into(), Value::Number(0.into()));
        m.insert("fail".into(), Value::Number(fail.into()));
        m.insert("state".into(), Value::String(state.into()));
        m
    };
    // 三行覆盖三种着色分支：完成（绿）/ 无需重传（灰）/ 读取失败（红，error 键缺失走 None 分支）
    let mut rows = vec![
        mk("filter_result.xlsx", 3, 3, 0, "完成（写回 12 格）"),
        mk("filter_result.xlsx", 0, 0, 0, "无需重传"),
        mk("filter_result.xlsx", 2, 0, 2, "读取失败"),
    ];
    // 第四种着色：跳过（缺字段）—— 非 OA3 判型的行，不算失败
    rows[2].insert("skipped".into(), Value::Number(2.into()));
    retry_table(ui, &p, &rows, follow, "uitest_retry_table", true)
}

/// 回归自证入口（--uitest 用）：切模式时左侧栏数据必须**按模式独立**。
/// 做法：给三个模式各自设一个专属目录 → 逐一切过去改成「本模式改过的值」→ 再切一轮，
/// 返回第二轮读到的目录。期望 ["D:/edited-scan","D:/edited-filter","D:/edited-retry"]。
pub fn mode_switch_probe(app: &mut FindanyApp) -> Vec<String> {
    let mut modes = [app.cfg.clone(), app.cfg.clone(), app.cfg.clone()];
    for (i, key) in ["scan", "filter", "retry"].iter().enumerate() {
        modes[i].root_dir = format!("D:/only-{key}");
        modes[i].work_mode = (*key).to_string();
    }
    app.set_mode_cfgs(modes);
    for m in [WorkMode::Scan, WorkMode::Filter, WorkMode::Retry] {
        app.switch_mode(m);
        app.cfg.root_dir = format!("D:/edited-{}", m.key());
    }
    let mut seen = Vec::new();
    for m in [WorkMode::Scan, WorkMode::Filter, WorkMode::Retry] {
        app.switch_mode(m);
        seen.push(app.cfg.root_dir.clone());
    }
    // 结果数据也要按模式独立：给「日志筛选」塞两行 → 切到扫描必须为空 → 切回来两行还在
    app.switch_mode(WorkMode::Filter);
    app.filter_items = vec![mode_probe_row("D:/probe-a.log"), mode_probe_row("D:/probe-b.log")];
    app.rebuild_filter_index();
    app.switch_mode(WorkMode::Scan);
    let scan_rows = app.filter_items.len();
    app.switch_mode(WorkMode::Filter);
    let back_rows = app.filter_items.len();
    seen.push(format!("rows_scan={scan_rows}_filter={back_rows}"));
    seen
}

fn mode_probe_row(path: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("rel_path".into(), Value::String(path.into()));
    m
}

/// ScanItem → 管道 Row（仅 uitest 构造测试数据用；运行时由 engine::scan_one 产出）
fn scan_item_to_row(it: &ScanItem) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("rel_path".into(), Value::String(it.rel_path.clone()));
    m.insert("dir_name".into(), Value::String(it.dir_name.clone()));
    m.insert("filename".into(), Value::String(it.filename.clone()));
    m.insert("hit".into(), Value::Bool(it.hit));
    m.insert(
        "hit_lines".into(),
        Value::String(it.hit_lines.iter().take(6).map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
    );
    m.insert("hit_line_text".into(), Value::String(it.hit_line_text.clone()));
    m.insert("hit_count".into(), Value::Number(it.hit_count.into()));
    m.insert("size_str".into(), Value::String(it.size_str()));
    m.insert("mtime_str".into(), Value::String(it.mtime_str()));
    m.insert("encoding".into(), Value::String(it.encoding.clone()));
    m
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

/// 后台跑「历史结果重传」：**逐个文件**读表 → 筛目标 → 逐台回传 → 写回原 xlsx。
/// 返回给人看的一句话（进状态栏与日志）；行级结果已写回各自的 Excel。
///
/// 在后台线程执行：逐台调 CLI 可能几十秒到几分钟，压在 UI 线程就是「点一下卡死」。
/// 单个文件失败不中断其余（末尾把失败的文件名列出来），否则一个坏表就挡住整批。
fn run_retry_job(
    paths: &[String],
    profile: &crate::core::logfilter::uploader::UploadProfile,
    dry_run: bool,
) -> anyhow::Result<String> {
    use crate::core::logfilter::retry;
    use crate::core::logfilter::uploader::{ST_CONFLICT, ST_DRY_RUN, ST_OK};
    let mut files_ok = 0usize;
    let mut files_fail: Vec<String> = Vec::new();
    let mut files_no_target = 0usize;
    let mut total_targets = 0usize;
    let mut total_skipped = 0usize;
    let mut ok = 0usize;
    let mut fail = 0usize;
    let mut conflict = 0usize;
    let mut dry = 0usize;
    let mut cells_written = 0usize;
    let mut backups = 0usize;

    for path in paths {
        let sheet = match retry::read_history(path) {
            Ok(s) => s,
            Err(e) => {
                files_fail.push(format!("{}（{e}）", file_basename(path)));
                continue;
            }
        };
        // 口径与界面重传一致：只看「判型 OA3 + 数据完整」，不看上次回传状态
        let (targets, skipped_rows) = retry::retry_targets(profile, &sheet);
        if targets.is_empty() && skipped_rows.is_empty() {
            files_no_target += 1;
            continue;
        }
        total_targets += targets.len();
        total_skipped += skipped_rows.len();
        // 不满足条件的先落表：跳过 + 原因
        let mut updates: Vec<(u32, serde_json::Map<String, serde_json::Value>)> = skipped_rows
            .iter()
            .map(|(r, why)| {
                let mut m = serde_json::Map::new();
                m.insert("upload_state".into(), Value::String(retry::ST_SKIP.into()));
                m.insert("upload_error".into(), Value::String(format!("跳过（{why}）")));
                m.insert("_status".into(), Value::String(retry::ST_SKIP.into()));
                (*r, m)
            })
            .collect();
        for row in sheet.rows.iter().filter(|r| targets.contains(&r.excel_row)) {
            let fields = retry::retry_one(profile, row, dry_run, &|_, _| {});
            match fields.get("_status").and_then(|v| v.as_str()).unwrap_or("") {
                ST_OK => ok += 1,
                ST_CONFLICT => conflict += 1,
                ST_DRY_RUN => dry += 1,
                _ => fail += 1,
            }
            updates.push((row.excel_row, fields));
        }
        match retry::write_back(path, &updates) {
            Ok((bak, cells)) => {
                files_ok += 1;
                cells_written += cells;
                if !bak.is_empty() {
                    backups += 1;
                }
            }
            Err(e) => files_fail.push(format!("{}（写回失败：{e}）", file_basename(path))),
        }
    }

    if total_targets == 0 && files_fail.is_empty() {
        return Ok(format!(
            "扫描了 {} 个 filter_result.xlsx：没有「判型 OA3 且数据完整」的行可回传（跳过 {total_skipped} 台）",
            paths.len()
        ));
    }
    Ok(format!(
        "历史结果重传完成：{} 个文件 / 共 {} 台（成功 {ok} / 冲突 {conflict} / 失败 {fail}{}）{}；写回 {cells_written} 个单元格，{backups} 个文件已备份{}{}",
        files_ok,
        total_targets,
        if dry > 0 { format!(" / dry-run {dry}") } else { String::new() },
        if total_skipped > 0 { format!("，跳过 {total_skipped} 台（非 OA3 或字段不全）") } else { String::new() },
        if files_no_target > 0 { format!("；{files_no_target} 个文件无需重传") } else { String::new() },
        if files_fail.is_empty() {
            String::new()
        } else {
            format!("；失败 {} 个：{}", files_fail.len(), files_fail.join("、"))
        }
    ))
}

/// 取文件名（错误提示里用）
fn file_basename(p: &str) -> String {
    crate::core::logfilter::retry::file_basename(p)
}

fn pick_file() -> Option<std::path::PathBuf> {
    rt().block_on(e_utils::dialog::a_sync::file())
}

/// **递归**收集目录下所有 findany 产出的 `filter_result.xlsx`（实现见 core::logfilter::retry）。
fn collect_filter_xlsx(root: &std::path::Path) -> Vec<String> {
    crate::core::logfilter::retry::collect_filter_xlsx(root)
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
        // 用 `cmd /c start "" "<dir>"` 而不是直接 spawn explorer.exe：
        // · `start` 是 cmd 内建，会立刻返回（不等待资源管理器窗口就绪）；
        // · 直接 spawn explorer 时，目标在网络盘/UNC 上会卡在 CreateProcess 的路径解析上，
        //   调用线程被占住几秒到几十秒（「打开目录就卡死」的成因之一）。
        // 空标题参数 "" 不能省：否则路径带引号时 start 会把它当窗口标题。
        return std::process::Command::new("cmd")
            .args(["/c", "start", "", &path.replace('/', "\\")])
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
/// 「加载更多」按钮的共用文案（通用扫描与日志筛选回传用同一套渲染）。
fn load_more_label(pool_len: usize, step: usize) -> String {
    if pool_len <= step {
        format!("加载更多（剩 {} 行）", pool_len)
    } else {
        format!("加载更多（剩 {} 行，点一次拉 {}）", pool_len, step)
    }
}

/// 通用结果表渲染：通用扫描与日志筛选回传共用这一份渲染实现。
/// 两个模式此前各有一份几乎逐行重复的表格代码 —— 这正是「筛选修了、扫描没修」的根源
/// （例如虚拟化、跟随最新、行缓存策略，改一处漏一处）。现在只保留这一份：
/// 调用方只提供「列定义」和「每格取什么文本/颜色」，其余（定高视口、横向滚动、
/// 纵向交给表格自身滚动区、跟随最新、虚拟化、uitest 探针）全部共用。
///
/// `cols`: (表头, 初始列宽, 最小列宽)
/// `cell(row, col, palette) -> (文本, 颜色)`
#[allow(clippy::type_complexity)]
fn rows_table(
    ui: &mut egui::Ui,
    p: &Palette,
    cols: &[(&str, f32, f32)],
    row_count: usize,
    follow: bool,
    scroll_id: &str,
    rows_changed: bool,
    // 这一列渲染成可点链接（点一下就用资源管理器打开该格的文本 = 目录路径）；None = 普通列
    link_col: Option<usize>,
    cell: &dyn Fn(usize, usize, &Palette) -> (String, Color32),
) -> (f32, f32, f32, f32) {
    // 供 --uitest 断言「跟随最新时每帧都精确贴底、且 offset 单调不减」
    let mut probe = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut rendered = 0usize;
    table_viewport(ui, |ui| {
        let vp = ui.max_rect().size();
        // 外层滚动区只管横向（列比窗口宽）；**纵向滚动必须用表格自己的滚动区**——
        // TableBuilder::body() 内部本身就是一个 ScrollArea（管行）。把纵向也交给外面这层时，
        // 外面那层的内容高恰好等于一屏，纵向范围恒为 0：
        //   · 每批新行都会把整张表顶上去再被夹回来 -> 画面一闪一闪
        //   · 表格永远不跟随最新行 -> 看不到实时预览
        let sa = egui::ScrollArea::horizontal().id_salt(scroll_id).auto_shrink([false, false]);
        sa.show(ui, |ui| {
            let mut tb = egui_extras::TableBuilder::new(ui)
                .vscroll(true)
                // 程序化滚动立即生效：平滑追赶动画 + 每 200ms 一批的刷新率 = 肉眼看到的"来回追"
                .animate_scrolling(false)
                .auto_shrink([false, false])
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for (_, w, min_w) in cols {
                tb = tb.column(egui_extras::Column::initial(*w).at_least(*min_w).clip(true));
            }
            if follow {
                tb = tb.stick_to_bottom(true);
                if rows_changed && row_count > 0 {
                    // 本帧有新行：把表格自己的滚动区钉到最后一行的底部
                    tb = tb.scroll_to_row(row_count - 1, Some(egui::Align::BOTTOM));
                }
            }
            let out = tb
                .header(ROW_H, |mut header| {
                    for (h, _, _) in cols {
                        header.col(|ui| {
                            ui.label(RichText::new(*h).size(SIZE_SMALL).strong().color(p.text2));
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, row_count, |mut row| {
                        rendered += 1;
                        let i = row.index();
                        for c in 0..cols.len() {
                            row.col(|ui| {
                                let (txt, color) = cell(i, c, p);
                                // 目录列 = 可点链接：点一下直接用资源管理器打开这个目录
                                if link_col == Some(c) && !txt.trim().is_empty() {
                                    let r = ui.add(
                                        egui::Label::new(
                                            RichText::new(txt.clone()).size(SIZE_BODY).color(p.brand).underline(),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    if r.clicked() {
                                        let _ = open_path(&txt);
                                    }
                                    r.on_hover_text("点击打开这个目录");
                                } else {
                                    ui.label(RichText::new(txt).size(SIZE_BODY).color(color));
                                }
                            });
                        }
                    });
                });
            // 探针：--uitest 用「跟随最新时每帧精确贴底 + 位置单调不减」锁住这个行为
            probe = (out.state.offset.y, (out.content_size.y - out.inner_rect.height()).max(0.0), out.content_size.y, out.inner_rect.height());
            if uitest_debug() {
                println!(
                    "[dbg] rows={} rendered={} vp={:?} offset={:?} content={:?} inner={:?}",
                    row_count, rendered, vp, out.state.offset, out.content_size, out.inner_rect.size()
                );
            }
        });
        vp
    });
    probe
}

/// Map 取字符串（空/缺省都返回空串，与既有取值口径一致）
fn sval(row: &Map<String, Value>, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// 通用扫描结果表：**数据来自与筛选完全相同的那条管道**（Row = Map），
/// 只是列定义与取值不同。渲染本身走 rows_table（同一个实现）。
#[allow(clippy::type_complexity)]
fn scan_table(ui: &mut egui::Ui, p: &Palette, rows: &[Map<String, Value>], follow: bool, scroll_id: &str, rows_changed: bool) -> (f32, f32, f32, f32) {
    const COLS: [(&str, f32, f32); 11] = [
        ("序号", 52.0, 52.0),
        ("相对路径", 280.0, 140.0),
        ("目录", 130.0, 70.0),
        ("扩展名", 66.0, 54.0),
        ("包含状态", 78.0, 54.0),
        ("命中行号", 120.0, 54.0),
        ("命中行内容", 340.0, 54.0),
        ("匹配计数", 76.0, 54.0),
        ("大小", 86.0, 54.0),
        ("修改时间", 160.0, 54.0),
        ("编码", 96.0, 54.0),
    ];
    rows_table(ui, p, &COLS, rows.len(), follow, scroll_id, rows_changed, Some(2), &|i, c, p| {
        let r = &rows[i];
        match c {
            0 => ((i + 1).to_string(), p.text),
            1 => (sval(r, "rel_path"), p.text),
            2 => (sval(r, "dir_name"), p.text),
            3 => (ext(&sval(r, "filename")), p.text),
            4 => {
                let hit = r.get("hit").and_then(|v| v.as_bool()).unwrap_or(false);
                let skip = sval(r, "skip_reason");
                if !skip.is_empty() {
                    ("跳过".to_string(), p.warn)
                } else if hit {
                    ("命中".to_string(), p.ok)
                } else {
                    ("未命中".to_string(), p.text2)
                }
            }
            5 => (sval(r, "hit_lines"), p.text),
            6 => (sval(r, "hit_line_text"), p.text),
            7 => (sval(r, "hit_count"), p.text),
            8 => (sval(r, "size_str"), p.text),
            9 => (sval(r, "mtime_str"), p.text),
            10 => (sval(r, "encoding"), p.text),
            _ => (String::new(), p.text),
        }
    })
}

/// 把 (表头, 取值键, 列宽) 的列定义转成 rows_table 需要的 (表头, 初始宽, 最小宽)
fn layout_of<'a>(cols: &[(&'a str, &str, f32)]) -> Vec<(&'a str, f32, f32)> {
    cols.iter().map(|(h, _, w)| (*h, *w, 54.0)).collect()
}

/// **历史结果重传**的结果表：每行 = 一个 `filter_result.xlsx` 文件的处理结果。
/// 与扫描/筛选同一套渲染（rows_table），只是列不同。
#[allow(clippy::type_complexity)]
fn retry_table(ui: &mut egui::Ui, p: &Palette, rows: &[Map<String, Value>], follow: bool, scroll_id: &str, rows_changed: bool) -> (f32, f32, f32, f32) {
    const COLS: [(&str, &str, f32); 11] = [
        ("序号", "idx", 52.0),
        ("文件", "file_name", 250.0),
        ("目录", "dir", 300.0),
        ("总行数", "rows_total", 76.0),
        ("待重传", "retry_targets", 76.0),
        ("成功", "ok", 62.0),
        ("冲突", "conflict", 62.0),
        ("失败", "fail", 62.0),
        ("跳过", "skipped", 62.0),
        ("状态", "state", 150.0),
        ("错误", "error", 260.0),
    ];
    rows_table(ui, p, &layout_of(&COLS), rows.len(), follow, scroll_id, rows_changed, Some(2), &|i, c, p| {
        if c == 0 {
            return ((i + 1).to_string(), p.text);
        }
        let key = COLS[c].1;
        let it = &rows[i];
        let v = match it.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        // 状态/计数着色：成功绿、失败红、跳过（缺字段）黄、无需重传灰
        let color = match key {
            "state" => {
                if v.starts_with("完成") {
                    p.ok
                } else if v.contains("无需") {
                    p.text2
                } else {
                    p.err
                }
            }
            "fail" if v != "0" && !v.is_empty() => p.err,
            "ok" if v != "0" && !v.is_empty() => p.ok,
            "skipped" if v != "0" && !v.is_empty() => p.warn,
            _ => p.text,
        };
        (truncate(&v, 80), color)
    })
}

#[allow(clippy::type_complexity)]
fn filter_table(ui: &mut egui::Ui, p: &Palette, items: &[Map<String, Value>], follow: bool, scroll_id: &str, rows_changed: bool) -> (f32, f32, f32, f32) {
    // (表头, 取值键, 初始列宽)；渲染全部走 rows_table（与通用扫描同一份实现）
    const COLS: [(&str, &str, f32); 14] = [
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
    let layout: Vec<(&str, f32, f32)> = layout_of(&COLS);
    rows_table(ui, p, &layout, items.len(), follow, scroll_id, rows_changed, None, &|i, c, p| {
        if c == 0 {
            return ((i + 1).to_string(), p.text);
        }
        let key = COLS[c].1;
        let it = &items[i];
        let v = match it.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let color = match key {
            "upload_state" | "extract_state" => match v.as_str() {
                "成功" => p.ok,
                "失败" => p.err,
                "冲突(人工)" => p.warn,
                _ => p.text,
            },
            _ => p.text,
        };
        (truncate(&v, 64), color)
    })
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
        let rows_now = self.filter_items.len();
        self.rows_changed = rows_now != self.last_row_count;
        self.last_row_count = rows_now;
        self.tick_save(&ctx);
        self.tick_countdown();
        if self.need_repaint {
            self.need_repaint = false;
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // **运行中必须持续醒来消费事件**（死锁根治）：
        // worker 用有界通道推送（通道满就阻塞等 UI 取走）；如果 UI 因为「这一帧没新数据」
        // 就不再要求重绘、egui 进入空闲，那就变成「worker 等 UI 消费 / UI 等 worker 出数据」——
        // 表现就是整个界面卡死、CPU 0%、所有线程 Wait（长时间跑必然撞上，跟数据量无关）。
        if self.running() || self.starting {
            ctx.request_repaint_after(Duration::from_millis(self.cfg.ui_refresh_ms.max(20) as u64));
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
        self.resume_window(&ctx, &p);

        // 自开始 / 自动化：首帧触发一次
        if (self.cfg.auto_start || self.auto_mode) && !self.running() && self.logs.len() <= 1 {
            self.push_log("info", if self.auto_mode { "自动化模式：启动即开跑（TOML）" } else { "run.auto_start=true，自动开跑" });
            if self.mode == WorkMode::Scan {
                self.start_scan();
            } else if self.mode == WorkMode::Retry {
                // 自动运行也要能跑「历史结果重传」——以前这里只有 scan/filter 两档，
                // 配了 work_mode=retry 的机器一开启自动运行就跑成了日志筛选（把输出目录当日志目录扫）
                self.start_retry();
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

/// 从多个候选位置找 assets/icon.png 作为窗口图标(覆盖 .exe 资源图标的运行时 HICON):
/// 1. exe 同级 assets/icon.png(zip 解压布局: findany.exe 与 assets/ 同目录)
/// 2. exe 同级 assets/etest-256.png(etest 品牌 fallback —— 不依赖 icon.png 是否就位)
/// 3. cwd/assets/icon.png(产机从快捷方式/服务方式启动时 cwd 可能不是 exe 目录)
/// 全部找不到 → 返回 None(viewport.with_icon 不调用 → 用 .exe 资源图标,即 winres 嵌入的 etest.ico,
/// 已经是新品牌图标 —— 保证窗口标题栏始终是新的)
fn load_window_icon() -> Option<eframe::egui::IconData> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let cwd = std::env::current_dir().ok();
    let candidates: [std::path::PathBuf; 4] = [
        exe_dir.join("assets").join("icon.png"),
        exe_dir.join("assets").join("etest-256.png"),
        cwd.as_deref().map(|c| c.join("assets").join("icon.png")).unwrap_or_default(),
        cwd.as_deref().map(|c| c.join("assets").join("etest-256.png")).unwrap_or_default(),
    ];
    for path in &candidates {
        if path.as_os_str().is_empty() { continue; }
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(icon) = eframe::icon_data::from_png_bytes(&bytes) {
                return Some(icon);
            }
        }
    }
    None
}

/// 启动 GUI（auto_mode=true 时首帧自动开跑并按 TOML 倒计时关窗）
pub fn run(cfg: SearchConfig, proc_dir: std::path::PathBuf, auto_mode: bool) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([1040.0, 660.0])
        .with_title("findany — 目录内容扫描器");
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "findany",
        native_options,
        Box::new(move |cc| {
            let font_note = theme::install_fonts(&cc.egui_ctx);
            let mut app = FindanyApp::new(cfg, proc_dir, font_note, auto_mode);
            // 三个模式的模版：左侧栏数据按模式独立（读文件里各自的 [modes.<模式>.*] 段）
            let path = crate::core::logfilter::autoconfig::config_path();
            app.set_mode_cfgs(crate::core::logfilter::autoconfig::load_mode_configs(&path));
            Ok(Box::new(app))
        }),
    )
}
