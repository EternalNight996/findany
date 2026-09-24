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
use std::path::Path;

// ============================================================================
// 集中进度节点：`<out_dir>/_resume.json`
//
// 主上要求「历史的上一次记录节点记录在当前 json 里，每次开始前读取并弹窗」。
// **三个模式各占一个槽**（scan / filter / retry）：切模式各有各的进度，互不覆盖
// （曾经只存一个节点 —— 扫描跑一半切到筛选，续跑弹窗弹的是扫描的进度，跳过集也是扫描的）。
//   · 开始时写入本模式的槽 / 每批刷新 done / 跑完清除本模式的槽
//   · 点开始前先读本模式的槽 —— 存在且未完成 → 弹窗问「继续上次 / 从头开始」
//
// 行数据（用于把上次的行载回表格）不放这里 —— 那可能几十 MB；
// 节点里只存 `data_file` 指针，指向 engine 边跑边写的 JSONL。
// ============================================================================

/// 集中进度节点（单个任务）
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
    /// **已跑完的文件夹**（绝对路径）：文件夹级续跑的核心 —— 续跑时整个目录直接跳过。
    /// 老节点没有这个键 → 空表（那次会从头跑一遍，安全）。
    #[serde(default)]
    pub done_dirs: Vec<String>,
}

/// 节点文件路径
pub fn node_path(out_dir: &str) -> String {
    let root = if out_dir.trim().is_empty() { "." } else { out_dir };
    Path::new(root).join("_resume.json").to_string_lossy().to_string()
}

/// 一个 out_dir 的进度节点文件（`_resume.json`）内容：**三个模式各一个槽**
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeStore {
    #[serde(default)]
    pub scan: Option<ResumeNode>,
    #[serde(default)]
    pub filter: Option<ResumeNode>,
    #[serde(default)]
    pub retry: Option<ResumeNode>,
}

impl ResumeStore {
    fn any(&self) -> bool {
        self.scan.is_some() || self.filter.is_some() || self.retry.is_some()
    }
    fn slot(&mut self, mode: &str) -> &mut Option<ResumeNode> {
        match mode {
            "scan" => &mut self.scan,
            "retry" => &mut self.retry,
            _ => &mut self.filter,
        }
    }
}

/// 读整份 store。兼容老格式（整份就是一个节点，槽字段缺失 → 再按单节点解析一次）
fn read_store(out_dir: &str) -> ResumeStore {
    let Ok(text) = std::fs::read_to_string(node_path(out_dir)) else {
        return ResumeStore::default();
    };
    if let Ok(s) = serde_json::from_str::<ResumeStore>(&text) {
        if s.any() {
            return s;
        }
    }
    match serde_json::from_str::<ResumeNode>(&text) {
        Ok(n) => {
            let mut s = ResumeStore::default();
            let mode = n.mode.clone();
            *s.slot(&mode) = Some(n);
            s
        }
        Err(_) => ResumeStore::default(),
    }
}

/// 只改本模式的槽，其余两个模式原样保留（写回整份）
fn write_slot(out_dir: &str, mode: &str, node: Option<&ResumeNode>) -> std::io::Result<()> {
    let mut store = read_store(out_dir);
    *store.slot(mode) = node.cloned();
    let p = node_path(out_dir);
    // 三个槽都空了：文件留着没意义，直接删掉更干净
    if !store.any() {
        let _ = std::fs::remove_file(&p);
        return Ok(());
    }
    if let Some(dir) = Path::new(&p).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let text = serde_json::to_string_pretty(&store).unwrap_or_default();
    // **原子写**：先写临时文件再 rename —— 中途被杀/断电不会留下半截 JSON 让状态错乱
    let tmp = format!("{p}.tmp-{}", std::process::id());
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &p)
}

/// 读本模式的节点：文件不存在 / 没有本模式的槽 / 已标记完成 / **空节点** 都返回 None（调用方直接开跑）
pub fn load_node(out_dir: &str, mode: &str) -> Option<ResumeNode> {
    let store = read_store(out_dir);
    let node = match mode {
        "scan" => store.scan,
        "retry" => store.retry,
        _ => store.filter,
    }?;
    // done >= total 且 total>0 → 视为已完成（正常跑完会 clear，这里是兜底）
    if node.total > 0 && node.done >= node.total {
        return None;
    }
    // 刚开始跑（还在遍历统计阶段）就被中断：total/done 都是 0 —— 一个文件都没处理过，
    // 没有任何可跳过的进度。这时弹「已处理 0/0」只会误导（点「继续」= 从头跑一遍），
    // 所以直接当没有未完成任务。
    if node.total == 0 && node.done == 0 {
        return None;
    }
    Some(node)
}

/// 写节点（开始时与每批刷新时调）—— 按 `node.mode` 落到对应槽
pub fn save_node(out_dir: &str, node: &ResumeNode) -> std::io::Result<()> {
    let mode = node.mode.clone();
    write_slot(out_dir, &mode, Some(node))
}

/// 清本模式的节点（跑完时调，避免下次被反复追问）；其他模式的进度不动
pub fn clear_node(out_dir: &str, mode: &str) {
    let _ = write_slot(out_dir, mode, None);
}

/// 人类可读的模式名（弹窗与日志用）
pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "scan" => "通用扫描",
        "retry" => "历史结果重传",
        _ => "日志筛选回传",
    }
}


/// 未完成的任务（上次中断留下的数据文件）
#[derive(Debug, Clone, Default)]
pub struct PendingRun {
    /// 数据文件路径（诊断用；续跑时 UI 直接用节点里的 data_file）
    pub path: String,
    /// 上次已落盘的行（可直接载回表格）
    pub rows: Vec<Map<String, Value>>,
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

/// 从数据文件（JSONL）读回已落盘的行 —— 续跑时把上次的行载回表格。
/// （**跳过判定不用它**：那是文件夹级，走节点里的 `done_dirs`。）
/// 空 / 不存在 / 没有完整行 → None（没有可恢复的东西）。
pub fn load_rows(data_file: &str) -> Option<PendingRun> {
    let path = data_file.to_string();
    if path.trim().is_empty() || !Path::new(&path).is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // 中断时可能写了半行 —— 跳过它，不影响其余
        };
        let Some(obj) = v.as_object() else { continue };
        rows.push(obj.clone());
    }
    if rows.is_empty() {
        return None;
    }
    Some(PendingRun { path, rows })
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
            skip_dirs: Default::default(),
            ..Default::default()
        }
    }

    #[test]
    fn node_roundtrip_and_clear() {
        let dir = std::env::temp_dir().join("findany-resume-node");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        assert!(load_node(&d, "filter").is_none(), "没写过节点时应为 None");
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
            done_dirs: vec!["D:/logs/a".into(), "D:/logs/b".into()],
        };
        save_node(&d, &node).unwrap();
        let back = load_node(&d, "filter").expect("应能读回");
        assert_eq!(back.mode, "filter");
        assert_eq!(back.done, 40);
        assert_eq!(back.total, 100);
        assert_eq!(back.done_dirs, vec!["D:/logs/a".to_string(), "D:/logs/b".to_string()], "文件夹级进度要能往返");
        // 跑完清掉 → 不该再被当成未完成
        clear_node(&d, "filter");
        assert!(load_node(&d, "filter").is_none(), "清掉后应为 None");
        // 已完成（done>=total）也算没有未完成
        let mut fin = node.clone();
        fin.done = fin.total;
        save_node(&d, &fin).unwrap();
        assert!(load_node(&d, "filter").is_none(), "已完成的任务不该提示续跑");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空节点（刚开始跑、还在统计阶段就被中断）：没有可恢复的进度，不许弹「继续上次」
    #[test]
    fn empty_node_is_not_pending() {
        // 目录名别和 empty_progress_file_is_not_pending 撞：两个测试并发跑同一个目录会互相删
        let dir = std::env::temp_dir().join("findany-resume-node-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        let mut n = ResumeNode { version: 1, mode: "filter".into(), total: 0, done: 0, ..Default::default() };
        save_node(&d, &n).unwrap();
        assert!(load_node(&d, "filter").is_none(), "0/0 的空节点不该提示续跑");
        // 真处理过文件（progress 有行）就必须能识别出来
        n.total = 20;
        n.done = 10;
        save_node(&d, &n).unwrap();
        assert_eq!(load_node(&d, "filter").unwrap().done, 10, "有进度的节点必须保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 三个模式各占一个槽：写一个模式不许把别的模式的进度挤掉/顶掉
    #[test]
    fn nodes_are_isolated_per_mode() {
        let dir = std::env::temp_dir().join("findany-resume-modes");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        let mk = |mode: &str, total: usize| ResumeNode {
            version: 1,
            mode: mode.into(),
            root_dir: format!("D:/{mode}"),
            out_dir: d.clone(),
            fingerprint: String::new(),
            total,
            done: 1,
            started_at: String::new(),
            updated_at: String::new(),
            data_file: String::new(),
            done_files: vec![],
            done_dirs: vec![],
        };
        save_node(&d, &mk("scan", 10)).unwrap();
        save_node(&d, &mk("filter", 20)).unwrap();
        save_node(&d, &mk("retry", 30)).unwrap();
        assert_eq!(load_node(&d, "scan").unwrap().total, 10, "扫描进度被别的模式覆盖了");
        assert_eq!(load_node(&d, "filter").unwrap().total, 20);
        assert_eq!(load_node(&d, "retry").unwrap().total, 30);
        // 清一个不动另外两个
        clear_node(&d, "filter");
        assert!(load_node(&d, "filter").is_none());
        assert_eq!(load_node(&d, "scan").unwrap().total, 10, "清筛选把扫描也清了");
        assert_eq!(load_node(&d, "retry").unwrap().total, 30);
        // 老格式（整份就是一个节点）也能读回
        let legacy = dir.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        let ld = legacy.to_string_lossy().to_string();
        std::fs::write(node_path(&ld), serde_json::to_string(&mk("scan", 7)).unwrap()).unwrap();
        assert_eq!(load_node(&ld, "scan").unwrap().total, 7, "老格式单节点应能按 mode 落到槽里");
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
    fn find_pending_reads_rows() {
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
        assert_eq!(p.rows.len(), 2, "两行已落盘的行都要能载回表格");
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
