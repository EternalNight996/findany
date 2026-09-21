//! 验收输出：etest / e-autotest 的 \`R<{json}>R\` 结果标记（对齐 etest-core 解析契约）。
//!
//! 契约（etest-core \`commond.rs::parse_rlog_last\` / e-utils \`cmd::rlog\`）：
//!   - 平台用正则 \`R<(?s:.*?)>R\` 取**最后一条**标记，JSON 反序列化为
//!     \`e_utils::cmd::CmdResult\`：\`{ content: string, status: bool, opts: T }\`
//!   - \`status == true\` 才算通过；\`status == false\` 时 \`content\` 即失败原因
//!   - \`R<\` 与 \`>R\` 必须同一行，行内不混其它文本
//!
//! 输出位置：
//!   stdout                    —— 运行末尾一行 R<...>R（平台抓 stdout 时用）
//!   logs/findany.log          —— **追加**一条（诊断日志 + 末尾 R 行，取最后一条即最新结论）
//!   logs/findany-result.log   —— 覆盖写，只留最新一条（is_check=true 的 APP 直接读结果文件）

use serde_json::json;

/// 滚动结果文件（只留最新一条；日志文件是 findany.log）
pub const RESULT_FILE: &str = "findany-result.log";
/// 运行日志（诊断行 + 末尾 R 行）
pub const LOG_FILE: &str = "findany.log";

/// 构造 e-autotest 风格 R<json>R（与 gpu-test build_rlog 同结构）
pub fn build_rlog(content: &str, status: bool, mode: &str) -> String {
    let json = json!({
        "content": content,
        "status": status,
        "opts": {
            "api": "None",
            "task": "",
            "init": false,
            "full": false,
            "filter": [],
            "args": [],
            "command": [],
            "mode": mode,
        }
    });
    format!("R<{json}>R")
}

fn logs_dir() -> std::path::PathBuf {
    let dir = crate::core::app_dir::app_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 覆盖写滚动结果文件（只留最新一条）
pub fn write_result_file(content: &str, status: bool, mode: &str) -> Option<std::path::PathBuf> {
    let path = logs_dir().join(RESULT_FILE);
    std::fs::write(&path, format!("{}\n", build_rlog(content, status, mode))).ok().map(|_| path)
}

/// 运行收尾：把 R 行**追加**到运行日志末尾（平台取最后一条即最新结论）。
/// 该行只有 \`R<{...}>R\` 本体、无任何前缀 —— 「扫描/筛选完最后一行即标准结果，可直接校验」。
pub fn append_to_log(content: &str, status: bool, mode: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    let path = logs_dir().join(LOG_FILE);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    let _ = writeln!(f, "{}", build_rlog(content, status, mode));
    Some(path)
}

/// 统一收尾入口：stdout 一行 + 日志末行 + 结果文件覆盖写。
///
/// 注：这是**唯一允许在 UI 线程同步做**的文件写（一次运行只写一次、几毫秒，而且 R 行必须
/// 在倒计时关窗/CLI 读结果之前落到文件里 —— 甩后台反而会和关窗竞速）。其它 I/O 一律走后台。
/// content 末尾自动补运行结束时间（R 行本体保持干净，时间信息在 JSON 内）。
pub fn emit(content: &str, status: bool, mode: &str) -> Option<std::path::PathBuf> {
    let stamped = format!("{}（{}）", content, chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let line = build_rlog(&stamped, status, mode);
    let _ = write_result_file(&stamped, status, mode);
    append_to_log(&stamped, status, mode);
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(format!("{line}\n").as_bytes());
    let _ = out.flush();
    Some(logs_dir().join(LOG_FILE))
}

/// 取文本里最后一条 R<...>R 并解析（语义与 etest-core parse_rlog_last 一致）
pub fn parse_last(text: &str) -> Option<(String, bool)> {
    let end = text.rfind(">R")?;
    let start = text[..end].rfind("R<")?;
    let v: serde_json::Value = serde_json::from_str(&text[start + 2..end]).ok()?;
    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
    let status = v.get("status").and_then(|s| s.as_bool()).unwrap_or(false);
    Some((content, status))
}
