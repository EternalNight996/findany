//! 程序目录与文件日志（5MB 轮转）。
//!
//! 日志分工：
//!   logs/findany-run.log  —— CLI / TOML 自动化运行的诊断日志
//!   logs/findany-gui.log  —— GUI 人工操作的诊断日志
//!   logs/findany.log      —— 运行日志（诊断行 + **末尾一条 R<...>R 结论**）
//!   logs/findany-result.log —— 只留最新一条 R<...>R（is_check 的 APP 直接读结果文件）
//!
//! 程序目录取当前可执行文件所在目录（与 Python 版取 sys.executable 的目录同义）。

use std::path::PathBuf;

/// UI 线程标识（GUI 首帧登记）。
/// 规矩：**UI 线程只读内存 + 画图，零 I/O**。任何阻塞 I/O 辅助函数进来先自证不在 UI 线程，
/// debug 版当场断言失败 —— 这样新加的 I/O 代码不可能悄悄又把界面卡死。
static UI_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// GUI 首帧调用一次，登记 UI 线程
pub fn mark_ui_thread() {
    let _ = UI_THREAD.set(std::thread::current().id());
}

/// 断言当前不在 UI 线程（只在 debug 版生效，release 零开销）
fn assert_bg(what: &str) {
    if let Some(id) = UI_THREAD.get() {
        debug_assert!(
            std::thread::current().id() != *id,
            "UI 线程里做了阻塞 I/O：{what} —— 这会把界面卡死，请挪到后台线程"
        );
    }
}

/// 程序目录（放 findany.toml / out / logs）。进程内只解析一次。
///
/// 优先级：环境变量 FINDANY_HOME > 可执行文件目录（可写时）> 用户目录
/// 安装包装到 Program Files / /usr/lib/findany 之后那目录对普通用户只读，
/// 配置/日志/产物写不进去 —— 这时自动落到用户目录，装完直接能用，不需要额外启动器。
pub fn app_dir() -> PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(resolve_app_dir).clone()
}

fn resolve_app_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("FINDANY_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        if writable(&d) {
            return d;
        }
        let fb = user_dir();
        let _ = std::fs::create_dir_all(&fb);
        return fb;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 真写一个探针文件再删掉：Windows 的权限位靠不住，实测才准
fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(".findany-write-probe");
    match std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 安装包装好后落配置/日志/产物的用户目录
fn user_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("findany")
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/share/findany"))
            .unwrap_or_else(std::env::temp_dir)
    }
}

/// 写一行文件日志（失败静默，与 Python 版一致：日志不可用不阻断业务）
pub fn log_line(name: &str, level: &str, msg: &str) {
    log_lines(name, &[(level.to_string(), msg.to_string())]);
}

/// 批量写日志：一次 open/append 写多行。
/// 界面每条日志单独 open/append/close 的话，一轮跑下来几千次文件操作会全压在 UI 线程上（卡顿源头）。
pub fn log_lines(name: &str, lines: &[(String, String)]) {
    if lines.is_empty() {
        return;
    }
    assert_bg("写日志文件");
    let dir = app_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(name);
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 5 * 1024 * 1024 {
        let _ = std::fs::rename(&path, dir.join(format!("{name}.1")));
    }
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut buf = String::new();
    for (level, msg) in lines {
        buf.push_str(&format!("{now} [{}] {msg}\r\n", level.to_uppercase()));
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(buf.as_bytes());
    }
}
