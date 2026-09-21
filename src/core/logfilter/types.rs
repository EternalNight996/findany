//! 日志类型判型（移植 sonar/logfilter/types.py，与 heg-admin-log parse_path_type 同序）。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogType {
    /// 仅作 GUI/配置「自动识别」占位，detect 结果不会是它
    Auto,
    EtestOa3,
    Etest,
    Eautotest,
    HegAutotest2,
    HegAutotest3,
    Unknown,
}

impl LogType {
    /// 与 Python `LogType.value` 完全一致的显示名（落 Excel / 配置比对都靠它）
    pub fn as_str(&self) -> &'static str {
        match self {
            LogType::Auto => "auto",
            LogType::EtestOa3 => "etest(OA3)",
            LogType::Etest => "etest",
            LogType::Eautotest => "e-autotest",
            LogType::HegAutotest2 => "海格旧测试2",
            LogType::HegAutotest3 => "海格旧测试3",
            LogType::Unknown => "未知",
        }
    }

    pub fn from_str(s: &str) -> Option<LogType> {
        Some(match s {
            "auto" => LogType::Auto,
            "etest(OA3)" => LogType::EtestOa3,
            "etest" => LogType::Etest,
            "e-autotest" => LogType::Eautotest,
            "海格旧测试2" => LogType::HegAutotest2,
            "海格旧测试3" => LogType::HegAutotest3,
            "未知" => LogType::Unknown,
            _ => return None,
        })
    }
}

pub const ANCHOR_OA3_START: &str = "OA3 inject Start";
pub const ANCHOR_HASH: &str = "<HardwareHash>";
pub const ANCHOR_TAIL_JSON: &str = "R<{";
pub const ANCHOR_AUTOTEST_LINE: &str = ": e-autotest";
/// heg-admin-log 同款前缀表（heg2 先于 heg3：IFT-START 须先于 IFT 匹配）
pub const HEG2_PREFIXES: [&str; 2] = ["IFT-START", "SN"];
pub const HEG3_PREFIXES: [&str; 7] = ["IFT", "CLEAN", "BURN", "FFT", "BATTERY", "BFT-SL", "BFT"];

/// 按定版优先级判型。path 仅取文件名前缀，text 为全文文本。
pub fn detect_log_type(path: &str, text: &str) -> LogType {
    let stem = file_name(path);
    let upper = stem.to_uppercase();
    if upper.starts_with("AUTO2") {
        return LogType::Eautotest;
    }
    if HEG2_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        return LogType::HegAutotest2;
    }
    if HEG3_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        return LogType::HegAutotest3;
    }
    if text.contains(ANCHOR_OA3_START) || text.contains(ANCHOR_HASH) {
        return LogType::EtestOa3;
    }
    for line in text.lines() {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if s.contains(ANCHOR_AUTOTEST_LINE) {
            return LogType::Eautotest;
        }
        break; // 只看首个非空行
    }
    if text.contains(ANCHOR_TAIL_JSON) {
        return LogType::Etest;
    }
    LogType::Unknown
}

/// 路径末段文件名（兼容 / 与 \\\\）
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// 所在目录名（末段目录）
pub fn dir_path(path: &str) -> String {
    match path.rsplit_once(['/', '\\']) {
        Some((d, _)) => d.to_string(),
        None => String::new(),
    }
}
