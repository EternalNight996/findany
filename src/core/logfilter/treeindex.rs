//! 目录树索引：**先把「有哪些目录、每个目录有多少匹配文件」统计清楚并落盘**，
//! 之后的处理阶段按这份清单走；统计完成标记落在 meta 里 —— **下次开始直接复用，不再重新统计**。
//!
//! 为什么单独一层：
//!   · 统计要遍历整棵树（大目录几分钟），它必须**可续**：已统计完的目录（连同其子孙）记在
//!     jsonl 里，下次跳过整棵子树，从断点接着数。
//!   · 处理阶段就靠这份清单判「还剩哪些目录」，中断损失只有正在跑的那一个目录。
//!   · 目录清单与文件数据分离：清单是 `_index_<指纹>.jsonl`（一行一个目录），
//!     行数据仍是产物本体 `_run_<指纹>.jsonl`。
//!
//! 文件布局（都在 out 目录下）：
//!   `_index_<指纹>.jsonl`      一行一个 `{"dir": "...", "files": N}`（追加式，O(1)）
//!   `_index_<指纹>.meta.json`  总数 + `completed` 标记（统计是否完整）

use super::engine::FilterRunCfg;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// 索引汇总（meta）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexMeta {
    pub version: u32,
    pub root: String,
    pub mode: String,
    /// 有匹配文件的目录数（统计单位）
    pub total_dirs: usize,
    /// 递归看到的子目录总数（含没有匹配文件的空目录）
    pub sub_dirs: usize,
    /// 匹配文件总数
    pub total_files: usize,
    /// **统计是否完整**：true = 下次开始直接复用，不再遍历
    pub completed: bool,
    pub started_at: String,
    pub updated_at: String,
}

/// 统计结果
#[derive(Debug, Clone, Default)]
pub struct TreeIndex {
    /// 有匹配文件的目录（绝对路径）→ 该目录下的匹配文件数
    pub dirs: Vec<(String, usize)>,
    pub meta: IndexMeta,
    /// 本次是否直接复用了已完成的索引（没重新遍历）
    pub reused: bool,
}

pub fn index_path(out_dir: &str, fp: &str) -> String {
    let root = if out_dir.trim().is_empty() { "." } else { out_dir };
    Path::new(root).join(format!("_index_{fp}.jsonl")).to_string_lossy().to_string()
}

pub fn meta_path(out_dir: &str, fp: &str) -> String {
    let root = if out_dir.trim().is_empty() { "." } else { out_dir };
    Path::new(root).join(format!("_index_{fp}.meta.json")).to_string_lossy().to_string()
}

/// 删掉索引（用户选「从头开始」/ 配置变了时用）
pub fn clear_index(out_dir: &str, fp: &str) {
    let _ = std::fs::remove_file(index_path(out_dir, fp));
    let _ = std::fs::remove_file(meta_path(out_dir, fp));
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 读索引 meta（`completed=true` = 已统计完整，下次开始直接复用）
pub fn load_meta(out_dir: &str, fp: &str) -> Option<IndexMeta> {
    let text = std::fs::read_to_string(meta_path(out_dir, fp)).ok()?;
    serde_json::from_str::<IndexMeta>(&text).ok()
}

/// 路径规范化（Windows 大小写不敏感、正反斜杠等价）——与续跑跳过集同一口径
fn norm_key(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

fn read_meta(out_dir: &str, fp: &str) -> Option<IndexMeta> {
    load_meta(out_dir, fp)
}

/// 读目录清单（一行一个；半截行跳过）
pub fn read_dirs(out_dir: &str, fp: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(index_path(out_dir, fp)) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let (Some(d), f) = (
            v.get("dir").and_then(|x| x.as_str()),
            v.get("files").and_then(|x| x.as_u64()).unwrap_or(0),
        ) else {
            continue;
        };
        out.push((d.to_string(), f as usize));
    }
    out
}

fn append_dir(ip: &str, dir: &str, files: usize) {
    use std::io::Write;
    if ip.is_empty() {
        return;
    }
    if let Some(p) = Path::new(ip).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(ip) {
        let _ = writeln!(f, "{{\"dir\":{},\"files\":{files}}}", serde_json::Value::String(dir.to_string()));
    }
}

fn save_meta(mp: &str, m: &IndexMeta) {
    if mp.is_empty() {
        return;
    }
    if let Some(p) = Path::new(mp).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(mp, serde_json::to_string_pretty(m).unwrap_or_default());
}

fn ext_ok(name: &str, exts: &[String]) -> bool {
    if exts.is_empty() {
        return true;
    }
    match name.rsplit_once('.') {
        Some((_, e)) => exts.iter().any(|x| x.eq_ignore_ascii_case(e)),
        None => false,
    }
}

fn name_ok(name: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    name.to_lowercase().contains(&needle.to_lowercase())
}

/// 统计（或复用）目录树索引。
///
/// · meta.completed=true → 直接读清单返回（`reused=true`），**一步遍历都不做**；
/// · 否则从上次的断点接着统计（已统计目录的整棵子树跳过），统计完写 completed=true 并落盘。
pub fn ensure(cfg: &FilterRunCfg, cancel: &AtomicBool) -> TreeIndex {
    let fp = super::resume::task_fingerprint(cfg);
    let out_dir = cfg.out_dir.clone();
    let ip = index_path(&out_dir, &fp);
    let mp = meta_path(&out_dir, &fp);

    // ① 上次已经统计完整 → 复用，不再遍历
    if let Some(m) = read_meta(&out_dir, &fp) {
        if m.completed {
            return TreeIndex { dirs: read_dirs(&out_dir, &fp), reused: true, meta: m };
        }
    }

    // ② 续统计
    let mut known: HashSet<String> = read_dirs(&out_dir, &fp).into_iter().map(|(d, _)| norm_key(&d)).collect();
    let mut meta = read_meta(&out_dir, &fp).unwrap_or_default();
    if meta.started_at.is_empty() {
        meta.started_at = now();
    }
    meta.version = 1;
    meta.root = cfg.root_dir.clone();
    meta.mode = cfg.mode.clone();
    meta.completed = false;
    meta.updated_at = now();
    save_meta(&mp, &meta);

    let exts = cfg.extensions.clone();
    let needle = cfg.name_filter.trim().to_string();
    let recursive = cfg.recursive;
    let mut stats = (meta.total_dirs, meta.sub_dirs, meta.total_files);
    let mut last_flush = std::time::Instant::now();

    // 单文件模式：root 指向一个文件时，它自己就是「一个目录下的一个文件」
    if Path::new(&cfg.root_dir).is_file() {
        let name = Path::new(&cfg.root_dir).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let n = usize::from(ext_ok(&name, &exts) && name_ok(&name, &needle));
        let dir = Path::new(&cfg.root_dir)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        append_dir(&ip, &dir, n);
        meta.total_dirs += 1;
        meta.total_files += n;
        meta.completed = true;
        meta.updated_at = now();
        save_meta(&mp, &meta);
        return TreeIndex { dirs: read_dirs(&out_dir, &fp), reused: false, meta };
    }

    let complete = walk(&cfg.root_dir, &exts, &needle, recursive, cancel, &ip, &mut known, &mut stats, &mut last_flush, &mp, &mut meta);
    meta.total_dirs = stats.0;
    meta.sub_dirs = stats.1;
    meta.total_files = stats.2;
    meta.completed = complete;
    meta.updated_at = now();
    save_meta(&mp, &meta);
    TreeIndex { dirs: read_dirs(&out_dir, &fp), reused: false, meta }
}

/// 深度优先统计；返回 true = 整棵子树统计完整（false = 被取消）
#[allow(clippy::too_many_arguments)]
fn walk(
    dir: &str,
    exts: &[String],
    needle: &str,
    recursive: bool,
    cancel: &AtomicBool,
    ip: &str,
    known: &mut HashSet<String>,
    stats: &mut (usize, usize, usize),
    last_flush: &mut std::time::Instant,
    mp: &str,
    meta: &mut IndexMeta,
) -> bool {
    if cancel.load(Ordering::SeqCst) {
        return false;
    }
    if known.contains(&norm_key(dir)) {
        return true; // 这棵子树上次统计完了，整棵跳过
    }
    let mut files = 0usize;
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in &entries {
            if cancel.load(Ordering::SeqCst) {
                return false;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                stats.1 += 1; // 子目录（含没有匹配文件的）
                if recursive {
                    let sub = p.to_string_lossy().to_string();
                    if !walk(&sub, exts, needle, recursive, cancel, ip, known, stats, last_flush, mp, meta) {
                        return false;
                    }
                }
            } else if ext_ok(&name, exts) && name_ok(&name, needle) {
                files += 1;
            }
        }
    }
    // 只把「有匹配文件的目录」写进清单：没文件的目录不必记（下次重走一下很便宜），
    // 这样清单条目 = 界面上的「有活干的目录数」，续跑判定也只看这些。
    if files > 0 {
        append_dir(ip, dir, files);
        known.insert(norm_key(dir));
        stats.0 += 1;
        stats.2 += files;
    }
    // 统计进度也要让界面动起来（大目录几分钟，不能像死了）：每 2 秒刷一次 meta
    if last_flush.elapsed().as_secs_f64() >= 2.0 {
        *last_flush = std::time::Instant::now();
        meta.total_dirs = stats.0;
        meta.sub_dirs = stats.1;
        meta.total_files = stats.2;
        meta.updated_at = now();
        save_meta(mp, meta);
    }
    true
}

/// **清单驱动**：按索引里的目录逐个列文件（不递归、不再走全树遍历），按 batch 回调。
///
/// 与 `scanner::walk_files_each` **回调语义完全一致**（返回 false = 立即停），
/// 所以处理逻辑可以原样复用，只是"文件从哪来"换成了清单 —— 大目录省掉一整遍全树遍历。
/// 代价（有意为之）：清单之后新增的目录不会再被发现；要包含新增内容就「从头开始」重新统计。
pub fn walk_dirs(
    dirs: &[(String, usize)],
    exts: &[String],
    name_filter: &str,
    batch: usize,
    on_chunk: &mut impl FnMut(&[String]) -> bool,
) {
    let batch = batch.max(1);
    let mut buf: Vec<String> = Vec::with_capacity(batch);
    for (dir, _) in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue }; // 目录被删了：跳过
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || e.path().is_dir() {
                continue;
            }
            if !ext_ok(&name, exts) || !name_ok(&name, name_filter) {
                continue;
            }
            buf.push(e.path().to_string_lossy().to_string());
            if buf.len() >= batch {
                let ok = on_chunk(&buf);
                buf.clear();
                if !ok {
                    return;
                }
            }
        }
    }
    if !buf.is_empty() {
        on_chunk(&buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_of(root: &str, out: &str) -> FilterRunCfg {
        FilterRunCfg {
            root_dir: root.into(),
            out_dir: out.into(),
            extensions: vec!["log".into()],
            ..Default::default()
        }
    }

    /// 统计 → 落盘 completed；**再调一次必须直接复用（不重新遍历）**
    #[test]
    fn index_completes_then_reuses() {
        let base = std::env::temp_dir().join("findany-treeindex-reuse");
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let out = base.join("out");
        for d in ["a", "b/c"] {
            std::fs::create_dir_all(src.join(d)).unwrap();
        }
        std::fs::write(src.join("a/1.log"), "x").unwrap();
        std::fs::write(src.join("a/2.log"), "x").unwrap();
        std::fs::write(src.join("b/c/3.log"), "x").unwrap();
        std::fs::write(src.join("a/skip.txt"), "x").unwrap();
        let c = cfg_of(&src.to_string_lossy(), &out.to_string_lossy());
        let cancel = AtomicBool::new(false);

        let i1 = ensure(&c, &cancel);
        assert!(!i1.reused, "第一次必须真统计");
        assert!(i1.meta.completed, "统计完整要打标记");
        assert_eq!(i1.meta.total_files, 3, "只认 .log");
        assert_eq!(i1.meta.total_dirs, 2, "只有 a 和 b/c 有匹配文件（清单只记有活干的目录）");
        assert_eq!(i1.meta.sub_dirs, 3, "递归看到的子目录：a / b / b/c");
        assert_eq!(i1.dirs.len(), 2, "meta 与清单条目数必须一致");
        assert!(i1.meta.sub_dirs >= 3, "子目录数要看得出来：{:?}", i1.meta.sub_dirs);

        // 改一下源目录（新加文件）：已完成的索引必须**不重新统计**，总数保持旧值
        std::fs::write(src.join("a/4.log"), "x").unwrap();
        let i2 = ensure(&c, &cancel);
        assert!(i2.reused, "completed 之后必须直接复用，不再遍历");
        assert_eq!(i2.meta.total_files, 3, "复用旧统计（不重新数）");
        assert_eq!(i2.dirs.len(), 2);

        // 清掉索引后重统计 → 看到新增文件
        clear_index(&out.to_string_lossy(), &super::super::resume::task_fingerprint(&c));
        let i3 = ensure(&c, &cancel);
        assert!(!i3.reused);
        assert_eq!(i3.meta.total_files, 4, "清了索引才重新统计");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 统计中断（取消）后不该标 completed；续统计要接着数
    #[test]
    fn cancelled_index_resumes() {
        let base = std::env::temp_dir().join("findany-treeindex-resume");
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let out = base.join("out");
        for d in ["a", "b", "c"] {
            std::fs::create_dir_all(src.join(d)).unwrap();
            std::fs::write(src.join(d).join("x.log"), "x").unwrap();
        }
        let c = cfg_of(&src.to_string_lossy(), &out.to_string_lossy());
        let cancel = AtomicBool::new(true); // 一上来就取消 → 统计不完整
        let i1 = ensure(&c, &cancel);
        assert!(!i1.meta.completed, "被取消就不能标 completed");
        assert_eq!(i1.meta.total_files, 0, "取消后什么都没数到");

        let cancel2 = AtomicBool::new(false);
        let i2 = ensure(&c, &cancel2);
        assert!(i2.meta.completed, "接着统计要能完成");
        assert_eq!(i2.meta.total_files, 3, "三个目录各一个文件");
        let _ = std::fs::remove_dir_all(&base);
    }
}
