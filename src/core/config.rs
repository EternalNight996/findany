//! 配置模型（唯一落盘形态：findany.toml；无 config.json）。
//!
//! 字段与分区对齐 Python 版 findany.toml 模板：findany.toml 既服务 GUI，
//! 也服务「检测 → 回传 → 倒计时关」自动化，一处改两处生效。
//! 不落盘的仅 `auto_start`（自动化开跑开关，在 [run]）；`work_mode` 会落盘，下次启动直接进上次的模式。

use serde::{Deserialize, Serialize};
use std::path::Path;

fn default_keyword() -> String {
    "IT6563".into()
}
fn default_threads() -> i64 {
    8
}
fn default_extensions() -> Vec<String> {
    ["txt", "log", "csv", "md", "xml", "json", "java", "cpp", "py", "ini", "cfg", "html"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
fn default_encoding() -> String {
    "auto".into()
}
fn default_max_file_mb() -> f64 {
    20.0
}
fn default_work_mode() -> String {
    "scan".into()
}
fn default_log_type() -> String {
    "auto".into()
}
fn default_ui_refresh_ms() -> i64 {
    200
}
fn default_upload_types() -> Vec<String> {
    vec!["etest(OA3)".into()]
}
fn default_upload_args() -> String {
    "upload --stdin --secret-key ~secret_key~".into()
}
fn default_upload_timeout() -> f64 {
    60.0
}
fn default_upload_retries() -> i64 {
    3
}
fn default_countdown() -> i64 {
    3
}
fn default_cache_capacity_rows() -> i64 {
    5000
}
fn default_true() -> bool {
    true
}

/// 一次扫描 / 筛选的完整配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    // ---------- [filter] ----------
    /// 扫描根目录；指向单个文件时按单文件处理
    pub root_dir: String,
    pub keyword: String,
    /// inc 包含 | exc 不包含
    pub mode: String,
    /// 并发线程 1..64
    pub threads: i64,
    pub extensions: Vec<String>,
    /// auto | utf-8 | gbk | gb2312 | utf-16 | latin-1 | ascii
    pub encoding: String,
    pub case_sensitive: bool,
    pub recursive: bool,
    /// 是否将命中文件复制到 out（筛选模式下 = 留存命中日志）
    pub copy_files: bool,
    /// 是否在 Excel 记录未命中文件
    pub record_miss: bool,
    /// 超过此大小视为二进制/超大，跳过
    pub max_file_mb: f64,
    /// 输出根目录，空=程序目录下的 out
    pub out_dir: String,
    /// auto | etest(OA3) | etest | e-autotest | 海格旧测试2 | 海格旧测试3
    pub log_type: String,
    /// 界面实时渲染间隔（毫秒）：每满这么久推一批结果给表格；0=不推送（仅结束时一次）
    pub ui_refresh_ms: i64,
    /// 文件名包含（子串，不分大小写）：空=不过滤。用来在目录里只挑名字带某段的文件
    pub name_filter: String,
    /// 内存上限（MB）：超过就主动安全停止；0=自动取物理内存的 90%
    pub mem_limit_mb: i64,
    /// 按 root_dir 的一级子目录分批跑（大目录推荐：内存只跟最大子目录有关）
    pub batch_dirs: bool,
    /// 只挑名字含这段的一级子目录（空=全部）
    pub batch_name_filter: String,
    /// 每批之间的休眠（毫秒）：服务器上跑时用它在 CPU/磁盘/网络盘之间让路；0=不让
    pub throttle_ms: i64,
    /// 最多处理多少个文件（0=不限）：防目录跑飞，超了记警告并按上限收尾
    pub max_files: i64,
    /// 行缓存容量（单位：条）：worker 与 UI 共同遵守的内存门限。
    /// 0=不限（保留全部）；>0 时插入超出立即淘汰最旧（FIFO），被淘汰的行暂留「LRU 池」，
    /// UI 表格底部「加载更多」可从池里拉回来查看（一次性全部拉回，不是逐条）。
    /// 默认 5000：百万级目录也不会让内存失控，UI 同时保持流畅。
    pub cache_capacity_rows: i64,

    // ---------- [upload] ----------
    pub enabled: bool,
    pub dry_run: bool,
    /// 参与回传的判型（自动模式下仅这些类型回传）
    pub types: Vec<String>,
    pub cli_path: String,
    pub secret_key: String,
    pub args: String,
    pub timeout_sec: f64,
    pub max_retries: i64,

    // ---------- [run] ----------
    /// 启动即自动开跑（GUI 与 --auto 共用）
    pub auto_start: bool,
    pub countdown_sec: i64,
    /// 倒计时归零自动关程序；关时只收提示
    pub auto_close: bool,
    /// 进程优先级：normal | below_normal | idle（服务器上别抢生产任务的资源；仅 Windows 生效）
    pub process_priority: String,

    // ---------- 界面 ----------
    /// 上次选择的工作模式：`scan` 通用扫描 | `filter` 日志筛选回传 | `retry` 历史结果重传。
    /// **落盘**（不再是运行期字段）：下次启动直接进上次用的那个模式的界面。
    pub work_mode: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            root_dir: String::new(),
            keyword: default_keyword(),
            mode: "inc".into(),
            threads: default_threads(),
            extensions: default_extensions(),
            encoding: default_encoding(),
            case_sensitive: false,
            recursive: true,
            copy_files: false,
            record_miss: true,
            max_file_mb: default_max_file_mb(),
            out_dir: String::new(),
            log_type: default_log_type(),
            ui_refresh_ms: default_ui_refresh_ms(),
            name_filter: String::new(),
            mem_limit_mb: 0,
            batch_dirs: false,
            batch_name_filter: String::new(),
            throttle_ms: 0,
            max_files: 0,
            cache_capacity_rows: default_cache_capacity_rows(),
            enabled: false,
            dry_run: true,
            types: default_upload_types(),
            cli_path: String::new(),
            secret_key: String::new(),
            args: default_upload_args(),
            timeout_sec: default_upload_timeout(),
            max_retries: default_upload_retries(),
            auto_start: false,
            countdown_sec: default_countdown(),
            auto_close: false,
            process_priority: "normal".into(),
            work_mode: default_work_mode(),
        }
    }
}

impl SearchConfig {
    /// 校验，返回错误列表（空=通过）。与 Python 版逐条对齐。
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let root = Path::new(&self.root_dir);
        if !self.root_dir.is_empty() && root.is_file() {
            // 单文件模式：指向文件即合法
        } else if self.root_dir.is_empty() || !root.is_dir() {
            errs.push("扫描目录/文件不存在".to_string());
        }
        if !(1..=64).contains(&self.threads) {
            errs.push("线程数需在 1~64 之间".to_string());
        }
        if !(0..=5000).contains(&self.throttle_ms) {
            errs.push("每批休眠需在 0~5000 毫秒之间".to_string());
        }
        if self.max_files < 0 {
            errs.push("文件数上限不能为负（0=不限）".to_string());
        }
        if !(0..=100_000).contains(&self.cache_capacity_rows) {
            errs.push("缓存行数需在 0~100000 之间（0=不限）".to_string());
        }
        if self.work_mode == "scan" {
            if self.keyword.trim().is_empty() {
                errs.push("关键字不能为空".to_string());
            }
            if self.mode != "inc" && self.mode != "exc" {
                errs.push("匹配模式不合法".to_string());
            }
            if self.encoding == "ascii" && self.keyword.chars().any(|c| c as u32 > 127) {
                errs.push("ASCII 编码无法匹配非 ASCII 关键字".to_string());
            }
        } else {
            // 非扫描档：日志筛选回传 或 历史结果重传
            if self.work_mode != "filter" && self.work_mode != "retry" {
                errs.push("工作模式不合法".to_string());
            }
            if self.enabled {
                if !self.dry_run && self.secret_key.trim().is_empty() {
                    errs.push("正式回传需填写 SecretKey（dry-run 可留空）".to_string());
                }
                if self.timeout_sec <= 0.0 {
                    errs.push("回传超时需大于 0 秒".to_string());
                }
                if !(0..=10).contains(&self.max_retries) {
                    errs.push("回传重试次数需在 0~10 之间".to_string());
                }
            }
            if !(3..=3600).contains(&self.countdown_sec) {
                errs.push("倒计时需在 3~3600 秒之间".to_string());
            }
            if !["normal", "below_normal", "idle"].contains(&self.process_priority.as_str()) {
                errs.push("进程优先级需是 normal / below_normal / idle".to_string());
            }
        }
        errs
    }

    /// 模板占位符决定 payload 走参数还是 stdin（对齐 Python：含 ~payload~ 走参数）
    pub fn use_stdin(&self) -> bool {
        !self.args.contains("~payload~")
    }

    /// 程序目录下的默认输出根
    pub fn resolve_out_dir(&mut self, app_dir: &Path) {
        if self.out_dir.trim().is_empty() {
            self.out_dir = crate::core::config::default_out_dir(app_dir).to_string_lossy().to_string();
        }
    }
}

/// 输出根目录：程序目录下的 out。
pub fn default_out_dir(app_dir: &Path) -> std::path::PathBuf {
    app_dir.join("out")
}

#[allow(dead_code)]
fn _keep_default_true() -> bool {
    default_true()
}
