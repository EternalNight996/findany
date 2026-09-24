//! findany.toml：**唯一配置文件**（读写都在这里，无 config.json）。
//!
//! 同一份 toml 服务三件事：
//!   GUI  —— 启动加载、保存合并回写（保留注释与手工开关）
//!   自动化 —— run.auto_start / run.countdown_sec / run.auto_close 驱动「检测→回传→倒计时关」
//!   无窗口 —— findany --auto 直接读它

use crate::core::config::SearchConfig;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------- 当前配置文件路径（GUI 保存用；未设置时回落到程序目录 findany.toml） ----------

fn slot() -> &'static Mutex<Option<PathBuf>> {
    static SLOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn set_config_path(p: &Path) {
    if let Ok(mut g) = slot().lock() {
        *g = Some(p.to_path_buf());
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(g) = slot().lock() {
        if let Some(p) = g.as_ref() {
            return p.clone();
        }
    }
    crate::core::app_dir::app_dir().join("findany.toml")
}

/// 命令行参数（对齐 Python argparse：只剩 --config）
#[derive(Debug, Default, Clone)]
pub struct CliArgs {
    pub config: String,
}

pub fn parse_args(argv: &[String]) -> CliArgs {
    let mut a = CliArgs::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" => {
                a.config = argv.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            _ => i += 1,
        }
    }
    a
}

/// 配置文件路径：显式 --config > 程序目录 findany.toml
pub fn resolve_path(cli: &CliArgs, app_dir: &str) -> std::path::PathBuf {
    if cli.config.is_empty() {
        std::path::Path::new(app_dir).join("findany.toml")
    } else {
        std::path::PathBuf::from(&cli.config)
    }
}

// ---------- 默认模板（与 justfile 的 toml_template 逐字一致） ----------

pub const DEFAULT_TOML: &str = r#"# findany 配置（唯一配置文件；GUI 与自动化共用，改完保存即生效）

[filter]
root_dir = ""                  # 方案一：SN 检索根目录；必填
log_type = "auto"              # auto | etest(OA3) | etest | e-autotest | 海格旧测试2 | 海格旧测试3
recursive = true
ui_refresh_ms = 200            # 界面实时渲染间隔(ms)：每满这么久推一批给表格；0=只在结束时出结果
name_filter = ""                # 文件名包含（子串，不分大小写）：空=不过滤（只挑名字带某段的文件）
mem_limit_mb = 0               # 内存上限(MB)：超过就主动安全停止（不会无声无息挂掉）；0=自动取物理内存的 90%
threads = 8                    # 并发线程 1~64：服务器上跑就调小（1~4），别抢生产任务
throttle_ms = 0                # 每批之间的休眠(ms, 0~5000)：给 CPU/磁盘/网络盘让路，服务器上建议 20~200
max_files = 0                  # 最多处理多少个文件(0=不限)：防目录跑飞
cache_capacity_rows = 5000      # 行缓存容量(0=不限)：超过立即淘汰最旧，UI「加载更多」可拉回

[run]
auto_start = false             # 改 true：启动即自动「检测→回传→倒计时关」
countdown_sec = 3              # 完成后倒计时，归零自动关程序
auto_close = true
process_priority = "normal"     # normal | below_normal | idle：服务器上建议 below_normal 或 idle（仅 Windows 生效）
work_mode = "filter"            # 上次用的工作模式：scan 通用扫描 | filter 日志筛选回传 | retry 历史结果重传
                                # 启动时直接进这个模式的界面（点保存配置时自动写回）

[upload]
enabled = true
dry_run = true                 # 先 true 演练（只组包不打网），无误后改 false
types = ["etest(OA3)"]         # 参与回传的判型
cli_path = ""                  # 空=程序目录下 intunehelper_cli.exe
secret_key = ""                # 正式回传必填；本文件勿提交仓库
args = "upload --stdin --secret-key ~secret_key~"
timeout_sec = 60
max_retries = 3

# ---------- 三个模式各自的模版（左侧栏的数据按模式独立存这里，互不影响） ----------
# 上面 [filter]/[upload]/[run] = 「当前模式」的镜像（findany --auto 与老工具只认它们）；
# 下面这些段由「保存配置」写入：切模式读各自那份，改哪个模式只动哪个模式。
[modes.scan.filter]
[modes.scan.upload]
[modes.scan.run]

[modes.filter.filter]
[modes.filter.upload]
[modes.filter.run]

[modes.retry.filter]
[modes.retry.upload]
[modes.retry.run]
"#;

/// 输出默认 toml；成功 true。已存在时不覆盖。
pub fn write_default(path: &Path) -> bool {
    if path.exists() {
        return false;
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    std::fs::write(path, DEFAULT_TOML).is_ok()
}

// ---------- 解析（toml 文本 → SearchConfig） ----------

fn table<'a>(doc: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    // 段名支持点分路径（`modes.scan.filter` 这种嵌套段），老的单层段名照旧
    let mut cur = doc;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    cur.is_table().then_some(cur)
}

fn str_of(doc: &toml::Value, section: &str, key: &str, def: &str) -> String {
    table(doc, section)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| def.to_string())
}

fn bool_of(doc: &toml::Value, section: &str, key: &str, def: bool) -> bool {
    table(doc, section).and_then(|t| t.get(key)).and_then(|v| v.as_bool()).unwrap_or(def)
}

fn int_of(doc: &toml::Value, section: &str, key: &str, def: i64) -> i64 {
    table(doc, section).and_then(|t| t.get(key)).and_then(|v| v.as_integer()).unwrap_or(def)
}

fn float_of(doc: &toml::Value, section: &str, key: &str, def: f64) -> f64 {
    table(doc, section)
        .and_then(|t| t.get(key))
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .unwrap_or(def)
}

fn str_list_of(doc: &toml::Value, section: &str, key: &str, def: &[&str]) -> Vec<String> {
    match table(doc, section).and_then(|t| t.get(key)).and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().map(|v| v.as_str().unwrap_or("").trim().to_string()).filter(|s| !s.is_empty()).collect(),
        None => def.iter().map(|s| s.to_string()).collect(),
    }
}

/// toml 文本 → SearchConfig（缺字段用默认；未知段/键忽略，兼容旧模板）
pub fn parse_config(text: &str) -> SearchConfig {
    let doc: toml::Value = text.parse().unwrap_or(toml::Value::Table(Default::default()));
    parse_at(&doc, "", &SearchConfig::default())
}

/// 解析某个模式的模版段：`[modes.<模式>.filter]` / `.upload` / `.run`。
/// 段不存在则整份回落到 base（老档升级后第一次：三个模式先都拿到顶层那份，之后各自独立）。
pub fn parse_mode(text: &str, mode_key: &str, base: &SearchConfig) -> SearchConfig {
    let doc: toml::Value = text.parse().unwrap_or(toml::Value::Table(Default::default()));
    parse_at(&doc, &format!("modes.{mode_key}"), base)
}

/// 段名前缀化：pre="" → `filter`；pre="modes.scan" → `modes.scan.filter`
fn sec(pre: &str, name: &str) -> String {
    if pre.is_empty() {
        name.to_string()
    } else {
        format!("{pre}.{name}")
    }
}

/// 按段前缀解析一份配置：pre="" 读顶层 [filter]/[upload]/[run]，
/// pre="modes.scan" 读 [modes.scan.*]（模式级没有 process_priority/work_mode，继承 base）
fn parse_at(doc: &toml::Value, pre: &str, base: &SearchConfig) -> SearchConfig {
    let f = sec(pre, "filter");
    let u = sec(pre, "upload");
    let r = sec(pre, "run");
    SearchConfig {
        root_dir: str_of(doc, &f, "root_dir", &base.root_dir),
        name_filter: str_of(doc, &f, "name_filter", &base.name_filter),
        mem_limit_mb: int_of(doc, &f, "mem_limit_mb", base.mem_limit_mb).max(0),
        batch_dirs: bool_of(doc, &f, "batch_dirs", base.batch_dirs),
        batch_name_filter: str_of(doc, &f, "batch_name_filter", &base.batch_name_filter),
        keyword: str_of(doc, &f, "keyword", &base.keyword),
        mode: str_of(doc, &f, "mode", &base.mode),
        threads: int_of(doc, &f, "threads", base.threads),
        extensions: str_list_of(
            doc,
            &f,
            "extensions",
            &base.extensions.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        ),
        encoding: str_of(doc, &f, "encoding", &base.encoding),
        case_sensitive: bool_of(doc, &f, "case_sensitive", base.case_sensitive),
        recursive: bool_of(doc, &f, "recursive", base.recursive),
        copy_files: bool_of(doc, &f, "copy_files", base.copy_files),
        record_miss: bool_of(doc, &f, "record_miss", base.record_miss),
        max_file_mb: float_of(doc, &f, "max_file_mb", base.max_file_mb),
        out_dir: str_of(doc, &f, "out_dir", &base.out_dir),
        ui_refresh_ms: int_of(doc, &f, "ui_refresh_ms", base.ui_refresh_ms).clamp(0, 5000),
        throttle_ms: int_of(doc, &f, "throttle_ms", base.throttle_ms).clamp(0, 5000),
        max_files: int_of(doc, &f, "max_files", base.max_files).max(0),
        cache_capacity_rows: int_of(doc, &f, "cache_capacity_rows", base.cache_capacity_rows).clamp(0, 100_000),
        log_type: str_of(doc, &f, "log_type", &base.log_type),
        enabled: bool_of(doc, &u, "enabled", base.enabled),
        dry_run: bool_of(doc, &u, "dry_run", base.dry_run),
        types: str_list_of(doc, &u, "types", &base.types.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
        cli_path: str_of(doc, &u, "cli_path", &base.cli_path),
        secret_key: str_of(doc, &u, "secret_key", &base.secret_key),
        args: str_of(doc, &u, "args", &base.args),
        timeout_sec: float_of(doc, &u, "timeout_sec", base.timeout_sec),
        max_retries: int_of(doc, &u, "max_retries", base.max_retries),
        auto_start: bool_of(doc, &r, "auto_start", base.auto_start),
        countdown_sec: int_of(doc, &r, "countdown_sec", base.countdown_sec),
        auto_close: bool_of(doc, &r, "auto_close", base.auto_close),
        process_priority: str_of(doc, &r, "process_priority", &base.process_priority),
        // 上次用的工作模式：启动时直接进这个模式的界面（scan / filter / retry）。
        // 非法值回落默认，避免手改 toml 写错就进不去界面。
        work_mode: {
            let v = str_of(doc, &r, "work_mode", &base.work_mode);
            match v.as_str() {
                "scan" | "filter" | "retry" => v,
                _ => base.work_mode.clone(),
            }
        },
    }
}

/// 从文件加载（文件不存在/解析失败时返回默认）。
///
/// **只读**：老档缺哪个受管键，就按默认值补在内存里（GUI 里每个选项照样可用），
/// 绝不回写文件 —— 只有 toml 不存在时才新建默认模板（ensure_config_file）。
/// 落盘只发生在一种情况：用户在 GUI 里点了「保存配置」。
pub fn load_config(path: &Path) -> SearchConfig {
    set_config_path(path);
    let Ok(text) = std::fs::read_to_string(path) else {
        return SearchConfig::default();
    };
    let mut cfg = parse_config(&text);
    if cfg.out_dir.is_empty() {
        cfg.out_dir = crate::core::config::default_out_dir(&crate::core::app_dir::app_dir())
            .to_string_lossy()
            .to_string();
    }
    cfg
}


// ---------- 写回（合并受管键，保留注释 / auto_start / 其余键） ----------

/// 三个模式各自的模版（顺序固定：**scan / filter / retry**）。
///
/// 左侧栏的数据按模式独立：切模式读各自那份、保存时各写各的，互不覆盖。
/// `[modes.<模式>.*]` 段不存在时，该模式整份回落到顶层（老档第一次升级就是这个状态）。
pub fn load_mode_configs(path: &Path) -> [SearchConfig; 3] {
    let base = load_config(path);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = [base.clone(), base.clone(), base.clone()];
    for (i, key) in ["scan", "filter", "retry"].iter().enumerate() {
        let mut c = parse_mode(&text, key, &base);
        if c.out_dir.is_empty() {
            c.out_dir = base.out_dir.clone();
        }
        c.work_mode = (*key).to_string();
        out[i] = c;
    }
    out
}


/// `filter` 段的受管键（检索条件 / 文件类型 / 资源控制）
fn filter_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("root_dir", "root_dir"),
        ("log_type", "log_type"),
        ("keyword", "keyword"),
        ("mode", "mode"),
        ("extensions", "extensions"),
        ("encoding", "encoding"),
        ("threads", "threads"),
        ("case_sensitive", "case_sensitive"),
        ("recursive", "recursive"),
        ("copy_files", "copy_files"),
        ("record_miss", "record_miss"),
        ("max_file_mb", "max_file_mb"),
        ("out_dir", "out_dir"),
        ("ui_refresh_ms", "ui_refresh_ms"),
        ("name_filter", "name_filter"),
        ("mem_limit_mb", "mem_limit_mb"),
        ("batch_dirs", "batch_dirs"),
        ("batch_name_filter", "batch_name_filter"),
        ("throttle_ms", "throttle_ms"),
        ("max_files", "max_files"),
        ("cache_capacity_rows", "cache_capacity_rows"),
    ]
}

/// `upload` 段的受管键（回传参数）
fn upload_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("enabled", "enabled"),
        ("dry_run", "dry_run"),
        ("types", "types"),
        ("cli_path", "cli_path"),
        ("secret_key", "secret_key"),
        ("args", "args"),
        ("timeout_sec", "timeout_sec"),
        ("max_retries", "max_retries"),
    ]
}

/// `run` 段的受管键（自动化开关 / 倒计时 / 进程优先级）
fn run_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("auto_start", "auto_start"),
        ("countdown_sec", "countdown_sec"),
        ("auto_close", "auto_close"),
        ("process_priority", "process_priority"),
        ("work_mode", "work_mode"),
    ]
}

/// 受管键表：[段] -> [(toml 键, SearchConfig 字段)]
///
/// 顶层三段 = **当前模式**的镜像（`findany --auto` 与老工具只认它们）；
/// `modes.<模式>.<段>` = 三个模式各自的模版（左侧栏数据按模式独立），互不影响。
fn managed_keys() -> Vec<(String, Vec<(&'static str, &'static str)>)> {
    let mut v: Vec<(String, Vec<(&'static str, &'static str)>)> = vec![
        ("filter".to_string(), filter_keys()),
        ("upload".to_string(), upload_keys()),
        ("run".to_string(), run_keys()),
    ];
    for m in ["scan", "filter", "retry"] {
        v.push((format!("modes.{m}.filter"), filter_keys()));
        v.push((format!("modes.{m}.upload"), upload_keys()));
        v.push((format!("modes.{m}.run"), run_keys()));
    }
    v
}

fn cfg_value(cfg: &SearchConfig, key: &str) -> Value {
    match key {
        "root_dir" => Value::String(cfg.root_dir.clone()),
        "log_type" => Value::String(cfg.log_type.clone()),
        "keyword" => Value::String(cfg.keyword.clone()),
        "mode" => Value::String(cfg.mode.clone()),
        "extensions" => Value::Array(cfg.extensions.iter().map(|e| Value::String(e.clone())).collect()),
        "encoding" => Value::String(cfg.encoding.clone()),
        "threads" => Value::Number(cfg.threads.into()),
        "case_sensitive" => Value::Bool(cfg.case_sensitive),
        "recursive" => Value::Bool(cfg.recursive),
        "copy_files" => Value::Bool(cfg.copy_files),
        "record_miss" => Value::Bool(cfg.record_miss),
        "max_file_mb" => Value::Number(serde_json::Number::from_f64(cfg.max_file_mb).unwrap_or_else(|| 20.into())),
        "out_dir" => Value::String(cfg.out_dir.clone()),
        "ui_refresh_ms" => Value::Number(cfg.ui_refresh_ms.into()),
        "name_filter" => Value::String(cfg.name_filter.clone()),
        "mem_limit_mb" => Value::Number(cfg.mem_limit_mb.into()),
        "batch_dirs" => Value::Bool(cfg.batch_dirs),
        "batch_name_filter" => Value::String(cfg.batch_name_filter.clone()),
        "throttle_ms" => Value::Number(cfg.throttle_ms.into()),
        "max_files" => Value::Number(cfg.max_files.into()),
        "cache_capacity_rows" => Value::Number(cfg.cache_capacity_rows.into()),
        "process_priority" => Value::String(cfg.process_priority.clone()),
        "work_mode" => Value::String(cfg.work_mode.clone()),
        "enabled" => Value::Bool(cfg.enabled),
        "dry_run" => Value::Bool(cfg.dry_run),
        "types" => Value::Array(cfg.types.iter().map(|t| Value::String(t.clone())).collect()),
        "cli_path" => Value::String(cfg.cli_path.clone()),
        "secret_key" => Value::String(cfg.secret_key.clone()),
        "args" => Value::String(cfg.args.clone()),
        "timeout_sec" => Value::Number(serde_json::Number::from_f64(cfg.timeout_sec).unwrap_or_else(|| 60.into())),
        "max_retries" => Value::Number(cfg.max_retries.into()),
        "auto_start" => Value::Bool(cfg.auto_start),
        "countdown_sec" => Value::Number(cfg.countdown_sec.into()),
        "auto_close" => Value::Bool(cfg.auto_close),
        _ => Value::Null,
    }
}

/// 值 → TOML 右值。字符串优先字面量串（Windows 路径免双反斜杠）。
fn toml_value(v: &Value) -> String {
    match v {
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else {
                n.as_f64().map(|f| format!("{f}")).unwrap_or_default()
            }
        }
        Value::Array(arr) => format!("[{}]", arr.iter().map(toml_value).collect::<Vec<_>>().join(", ")),
        Value::Null => "''".to_string(),
        other => {
            let s = other.as_str().map(|x| x.to_string()).unwrap_or_else(|| other.to_string());
            if !s.contains('\'') && !s.contains('\n') {
                format!("'{s}'")
            } else {
                let esc = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
                format!("\"{esc}\"")
            }
        }
    }
}

fn set_last(v: &mut Vec<(String, usize)>, section: &str, idx: usize) {
    match v.iter_mut().find(|(s, _)| s == section) {
        Some((_, i)) => *i = idx,
        None => v.push((section.to_string(), idx)),
    }
}

/// 段名 → 用哪一份配置：`modes.<模式>.<段>` 用该模式的模版；顶层 filter/upload/run 用当前模式那份。
/// （顶层段名的第二段不存在 → 落到 current）
fn cfg_for_section<'a>(sec: &str, current: &'a SearchConfig, modes: &'a [SearchConfig; 3]) -> &'a SearchConfig {
    match sec.split('.').nth(1) {
        Some("scan") => &modes[0],
        Some("filter") => &modes[1],
        Some("retry") => &modes[2],
        _ => current,
    }
}

/// 把配置合并回 toml：受管键改右值，缺键补齐到对应段末尾；
/// 注释、空行、run.auto_start 与其它未管键原样保留。返回写入路径。
pub fn save_config(path: &Path, cfg: &SearchConfig) -> std::io::Result<String> {
    // 兼容入口（自动化 / 测试）：只按一份配置写，三个模式段与顶层取同一个值
    let modes = [cfg.clone(), cfg.clone(), cfg.clone()];
    save_config_all(path, cfg, &modes)
}

/// 保存：顶层三段 = `current`（当前模式）的镜像，另加三个模式各自的模版段。
pub fn save_config_all(path: &Path, current: &SearchConfig, modes: &[SearchConfig; 3]) -> std::io::Result<String> {
    if !path.exists() {
        write_default(path);
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.split('\n').collect();

    let mut section = String::new();
    let mut out: Vec<String> = Vec::new();
    let mut written: Vec<(String, String)> = Vec::new();
    let mut last_idx: Vec<(String, usize)> = Vec::new();

    let value_for = |sec: &str, key: &str| -> Option<Value> {
        let cfg = cfg_for_section(sec, current, modes);
        for (s, keys) in managed_keys() {
            if s != sec {
                continue;
            }
            for (tk, ck) in keys {
                if tk == key {
                    return Some(cfg_value(cfg, ck));
                }
            }
        }
        None
    };

    for ln in &lines {
        let s = ln.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s[1..s.len() - 1].trim().to_string();
        }
        if !s.starts_with('#') {
            if let Some((key, _)) = s.split_once('=') {
                let key = key.trim().to_string();
                if let Some(v) = value_for(&section, &key) {
                    let comment = match s.find('#') {
                        Some(h) => format!("  {}", &s[h..]),
                        None => String::new(),
                    };
                    out.push(format!("{key} = {}{comment}", toml_value(&v)));
                    written.push((section.clone(), key));
                    set_last(&mut last_idx, &section, out.len() - 1);
                    continue;
                }
            }
        }
        out.push((*ln).to_string());
        if !section.is_empty() {
            set_last(&mut last_idx, &section, out.len() - 1);
        }
    }

    // 缺键补齐：插到该段最后一行之后（自下而上，索引不漂移）
    let mut missing: Vec<(Vec<String>, usize)> = Vec::new();
    let mut new_sections: Vec<(String, Vec<String>)> = Vec::new();
    for (sec, keys) in managed_keys() {
        let mut adds: Vec<String> = Vec::new();
        for (tk, _) in keys {
            if written.iter().any(|(s, k)| s == &sec && k == tk) {
                continue;
            }
            if let Some(v) = value_for(&sec, tk) {
                adds.push(format!("{tk} = {}", toml_value(&v)));
            }
        }
        if adds.is_empty() {
            continue;
        }
        match last_idx.iter().find(|(s, _)| s == &sec) {
            Some((_, i)) => missing.push((adds, *i)),
            // 整段缺失（老档没有 [upload] 这类）：在文件末尾补出整段
            None => new_sections.push((sec, adds)),
        }
    }
    missing.sort_by_key(|(_, i)| std::cmp::Reverse(*i));
    for (adds, idx) in missing {
        for (offset, line) in adds.into_iter().enumerate() {
            out.insert(idx + 1 + offset, line);
        }
    }
    for (sec, adds) in new_sections {
        if out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
            out.push(String::new());
        }
        out.push(format!("[{sec}]"));
        out.extend(adds);
    }

    let joined = out.join("\n");
    std::fs::write(path, &joined)?;
    Ok(path.to_string_lossy().to_string())
}

/// 需要时生成默认模板（GUI 首次启动）
pub fn ensure_config_file(path: &Path) -> bool {
    if path.exists() {
        return false;
    }
    write_default(path)
}
