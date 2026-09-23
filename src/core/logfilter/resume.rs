//! 断点续扫：找未完成的任务、把上次已落盘的行与已处理路径读回来。
//!
//! 机制：进度文件（JSONL）**就是产物本体** —— engine 边跑边追加，跑完（未取消时）会删掉它。
//! 所以「文件存在 且 有内容」就等价于「上次没跑完，可以续」。
//!
//! 为什么不用单独的「进度标记」文件：两份状态很容易不一致（进度说完成、产物却是半截）。
//! 让产物自己当进度，天然只有一个真相。

use super::engine::FilterRunCfg;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::Path;

// ============================================================================
// 集中进度节点：`<out_dir>/_resume.json`
//
// 主上要求「历史的上一次记录节点记录在当前 json 里，每次开始前读取并弹窗」。
// 这里**只记当前这一个任务**（不是每个任务一份），三种模式（scan / filter / retry）共用同一份：
//   · 开始时写入 / 每批刷新 done / 跑完清除
//   · 点开始前先读它 —— 存在且未完成 → 弹窗问「继续上次 / 从头开始」
//
// 行数据（用于把上次的行载回表格）不放这里 —— 那可能几十 MB；
// 节点里只存 `data_file` 指针，指向 engine 边跑边写的 JSONL。
// ============================================================================

/// 集中进度节点（唯一一份，代表「上一次的任务」）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResumeNode {
    /// 结构版本（将来改字段好做兼容）
    pub version: u32,
    /// `scan` | `filter` | `retry`
    pub mode: String,
    /// 扫描/筛选：扫描根目录；retry：用户选的待重传目录
    pub root_dir: String,
    /// 输出目录（数据文件与产物都在这里）
    pub out_dir: String,
    /// 任务指纹（scan/filter 用；retry 记根目录的指纹）
    pub fingerprint: String,
    /// 总数（scan/filter＝本次要处理的文件数；retry＝找到的 xlsx 个数）
    pub total: usize,
    /// 已完成（scan/filter＝已落盘行数；retry＝已处理完的 xlsx 数）
    pub done: usize,
    pub started_at: String,
    pub updated_at: String,
    /// 行数据文件（scan/filter 的 JSONL 路径；retry 为空）
    pub data_file: String,
    /// retry 专用：已处理完的 xlsx 绝对路径（文件级续跑）
    pub done_files: Vec<String>,
}

/// 节点文件路径
pub fn node_path(out_dir: &str) -> String {
    let root = if out_dir.trim().is_empty() { "." } else { out_dir };
    Path::new(root).join("_resume.json").to_string_lossy().to_string()
}

/// 读节点：文件不存在 / 解析失败 / 已标记完成 都返回 None（调用方直接开跑）
pub fn load_node(out_dir: &str) -> Option<ResumeNode> {
    let p = node_path(out_dir);
    let text = std::fs::read_to_string(&p).ok()?;
    let node: ResumeNode = serde_json::from_str(&text).ok()?;
    // done >= total 且 total>0 → 视为已完成（正常跑完会 clear，这里是兜底）
    if node.total > 0 && node.done >= node.total {
        return None;
    }
    Some(node)
}

/// 写节点（开始时与每批刷新时调）
pub fn save_node(out_dir: &str, node: &ResumeNode) -> std::io::Result<()> {
    let p = node_path(out_dir);
    if let Some(dir) = Path::new(&p).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let text = serde_json::to_string_pretty(node).unwrap_or_default();
    std::fs::write(&p, text)
}

/// 清节点（跑完时调，避免下次被反复追问）
pub fn clear_node(out_dir: &str) {
    let _ = std::fs::remove_file(node_path(out_dir));
}

/// 人类可读的模式名（弹窗与日志用）
pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "scan" => "通用扫描",
        "retry" => "历史结果重传",
        _ => "日志筛选回传",
    }
}


/// 未完成的任务（上次中断留下的进度文件）
#[derive(Debug, Clone, Default)]
pub struct PendingRun {
    /// 数据文件路径（诊断用；续跑时 UI 直接用节点里的 data_file）
    #[allow(dead_code)]
    pub path: String,
    /// 上次已落盘的行（可直接载回表格）
    pub rows: Vec<Map<String, Value>>,
    /// 已处理过的文件绝对路径（续跑时跳过）
    pub done_paths: HashSet<String>,
}

impl PendingRun {
    /// 已处理的行数（给弹窗显示）
    pub fn done_rows(&self) -> usize {
        self.rows.len()
    }
}

/// 任务指纹：只有「同一批文件」才算同一个任务，避免把别的任务的进度认成自己的。
/// 组成：root_dir + mode + 扩展名集合（排序后）+ 文件名过滤。
pub fn task_fingerprint(cfg: &FilterRunCfg) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(cfg.root_dir.as_bytes());
    h.update(b"\x1f");
    h.update(cfg.mode.as_bytes());
    h.update(b"\x1f");
    let mut exts = cfg.extensions.clone();
    exts.sort(); // 扩展名顺序不同但集合相同，应视为同一任务
    for e in &exts {
        h.update(e.as_bytes());
        h.update(b",");
    }
    h.update(b"\x1f");
    h.update(cfg.name_filter.as_bytes());
    let d = format!("{:x}", h.finalize());
    d[..12].to_string()
}

/// 进度文件路径：与产物同目录（用户能找到，也跟着产物一起被清理）
pub fn progress_path(out_dir: &str, fingerprint: &str) -> String {
    let root = if out_dir.trim().is_empty() { "." } else { out_dir };
    Path::new(root).join(format!("_run_{fingerprint}.jsonl")).to_string_lossy().to_string()
}

/// 从数据文件（JSONL）读回已落盘的行与已处理路径。
/// 续跑时：行载回表格、`done_paths` 用来跳过已处理的文件。
/// 空/不存在/没有完整行 → None（没有可恢复的东西）。
pub fn load_rows(data_file: &str) -> Option<PendingRun> {
    let path = data_file.to_string();
    if path.trim().is_empty() || !Path::new(&path).is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let mut rows = Vec::new();
    let mut done_paths = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // 中断时可能写了半行 —— 跳过它，不影响其余
        };
        let Some(obj) = v.as_object() else { continue };
        if let Some(p) = obj.get("abs_path").and_then(|x| x.as_str()) {
            if !p.is_empty() {
                done_paths.insert(p.to_string());
            }
        }
        rows.push(obj.clone());
    }
    if rows.is_empty() {
        return None;
    }
    Some(PendingRun { path, rows, done_paths })
}

/// 找未完成任务（按 out_dir + 指纹推数据文件路径）——`load_rows` 的便捷包装
pub fn find_pending(out_dir: &str, fingerprint: &str) -> Option<PendingRun> {
    load_rows(&progress_path(out_dir, fingerprint))
}

/// 丢弃未完成任务（用户选「从头开始」时调）
#[allow(dead_code)]
pub fn discard(pending: &PendingRun) {
    let _ = std::fs::remove_file(&pending.path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_of(root: &str, mode: &str, exts: &[&str], name: &str) -> FilterRunCfg {
        FilterRunCfg {
            root_dir: root.into(),
            mode: mode.into(),
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            name_filter: name.into(),
            progress_path: String::new(),
            skip_paths: Default::default(),
            ..Default::default()
        }
    }

    #[test]
    fn node_roundtrip_and_clear() {
        let dir = std::env::temp_dir().join("findany-resume-node");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        assert!(load_node(&d).is_none(), "没写过节点时应为 None");
        let node = ResumeNode {
            version: 1,
            mode: "filter".into(),
            root_dir: "D:/logs".into(),
            out_dir: d.clone(),
            fingerprint: "abc123".into(),
            total: 100,
            done: 40,
            started_at: "2026-09-23 10:00:00".into(),
            updated_at: "2026-09-23 10:05:00".into(),
            data_file: "D:/out/_run_abc123.jsonl".into(),
            done_files: vec![],
        };
        save_node(&d, &node).unwrap();
        let back = load_node(&d).expect("应能读回");
        assert_eq!(back.mode, "filter");
        assert_eq!(back.done, 40);
        assert_eq!(back.total, 100);
        // 跑完清掉 → 不该再被当成未完成
        clear_node(&d);
        assert!(load_node(&d).is_none(), "清掉后应为 None");
        // 已完成（done>=total）也算没有未完成
        let mut fin = node.clone();
        fin.done = fin.total;
        save_node(&d, &fin).unwrap();
        assert!(load_node(&d).is_none(), "已完成的任务不该提示续跑");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mode_label_covers_all_three() {
        assert_eq!(mode_label("scan"), "通用扫描");
        assert_eq!(mode_label("filter"), "日志筛选回传");
        assert_eq!(mode_label("retry"), "历史结果重传");
        assert_eq!(mode_label("other"), "日志筛选回传", "未知模式兜底为筛选");
    }

    #[test]
    fn fingerprint_is_stable_and_extension_order_independent() {
        let a = cfg_of("D:/logs", "filter", &["log", "txt"], "MT71");
        let b = cfg_of("D:/logs", "filter", &["txt", "log"], "MT71"); // 顺序不同
        assert_eq!(task_fingerprint(&a), task_fingerprint(&b), "扩展名顺序不应影响指纹");
    }

    #[test]
    fn fingerprint_differs_on_semantics() {
        let base = cfg_of("D:/logs", "filter", &["log"], "");
        assert_ne!(task_fingerprint(&base), task_fingerprint(&cfg_of("D:/other", "filter", &["log"], "")), "目录不同");
        assert_ne!(task_fingerprint(&base), task_fingerprint(&cfg_of("D:/logs", "scan", &["log"], "")), "模式不同");
        assert_ne!(task_fingerprint(&base), task_fingerprint(&cfg_of("D:/logs", "filter", &["txt"], "")), "扩展名不同");
        assert_ne!(task_fingerprint(&base), task_fingerprint(&cfg_of("D:/logs", "filter", &["log"], "MT")), "名过滤不同");
    }

    #[test]
    fn find_pending_reads_rows_and_done_paths() {
        let dir = std::env::temp_dir().join("findany-resume-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = cfg_of("D:/logs", "filter", &["log"], "");
        let fp = task_fingerprint(&c);
        let path = progress_path(&dir.to_string_lossy(), &fp);
        // 写两行进度（含一行「半截 JSON」模拟中断）
        std::fs::write(
            &path,
            "{\"abs_path\":\"D:/logs/a.log\",\"sn\":\"SN-A\"}\n\
             {\"abs_path\":\"D:/logs/b.log\",\"sn\":\"SN-B\"}\n\
             {\"abs_path\":\"D:/logs/c.log\",\"sn\":\"半\n",
        )
        .unwrap();
        let p = find_pending(&dir.to_string_lossy(), &fp).expect("应能识别未完成任务");
        assert_eq!(p.done_rows(), 2, "半截那一行应被跳过");
        assert!(p.done_paths.contains("D:/logs/a.log"));
        assert!(p.done_paths.contains("D:/logs/b.log"));
        assert!(!p.done_paths.contains("D:/logs/c.log"), "没写完的行不算已完成");
        // 丢弃后应找不到
        discard(&p);
        assert!(find_pending(&dir.to_string_lossy(), &fp).is_none(), "丢弃后不应再有未完成任务");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_progress_file_is_not_pending() {
        let dir = std::env::temp_dir().join("findany-resume-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = cfg_of("D:/logs", "scan", &["log"], "");
        let fp = task_fingerprint(&c);
        std::fs::write(progress_path(&dir.to_string_lossy(), &fp), "").unwrap();
        assert!(find_pending(&dir.to_string_lossy(), &fp).is_none(), "空进度文件不算未完成（无可恢复）");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
