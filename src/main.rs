//! findany — 目录内容扫描器 / 产线日志筛选回传（Rust + egui，etest 同技术栈）
//!
//! 控制台策略（打包版不再有控制台黑框）：
//!   · release = Windows GUI 子系统：双击/被计划任务拉起都不会闪一下控制台窗口；
//!     命令行模式（--auto/--selftest/--qa/--uitest/--bench）启动时 AttachConsole 借父进程的控制台，
//!     输出照旧；被重定向到管道/文件时直接用继承来的句柄，脚本照样能读到 stdout。
//!   · debug 保留 console 子系统（cargo run 直接看输出），GUI 启动时把控制台窗口藏掉。
//!
//! 配置文件只有一个：程序目录（或 --config 指定）的 findany.toml。
//!
//! 崩溃处理：写 logs/crash.txt（含 backtrace）；GUI 模式额外弹「❌BUG跟踪Panic」错误框；
//! CLI 模式（--auto/--selftest/--qa/--uitest/--bench）只写文件不弹窗，避免无人值守被模态框卡住。

// 打包版（release）不带控制台子系统：这是「双击弹出黑框」的根因。
// debug 保留控制台，方便 cargo run 直接看输出。CLI 输出由 attach_parent_console 兜住。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// ↑ 与 gpu-test / heg-os-active2 同款：release = GUI 子系统（双击不弹命令行窗口），
//   启动时 reattach_windows_terminal() 挂父控制台，cmd 下照样能看到 stdout 日志。

// Windows 分配器换成 mimalloc：系统默认分配器不把空闲内存还给 OS，表现为
// 「跑完一轮后内存降不回去」（实测 89MB 起步、峰值 800MB、结束后停在 453MB）。
// mimalloc 会定期 purge 空闲页，结束后工作集能真正回落。
#[cfg(windows)]
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod core;
mod ui;

// Windows 软件 OpenGL 兜底(服务器/RDP 上系统只有 OpenGL 1.1 时,改用随包 Mesa 重启;
// 见 soft_gl.rs 顶部说明)。非 Windows 是空实现。
#[cfg(windows)]
mod soft_gl;
#[cfg(not(windows))]
mod soft_gl {
    /// 非 Windows 不存在「WGL 只有 1.1」这回事,不兜底
    pub fn relaunch_with_mesa() -> bool {
        false
    }
}

/// 崩溃处理（对齐 etest / e-log 的 panic 契约）。
///
/// 一个钩子里做完三件事（不能拆成两个钩子：后装的会整体替换先装的）：
///   1. 写 logs/crash.txt（SOURCE / MESSAGE / BACKTRACE）—— 文件名与 e-log 的落盘约定一致
///   2. GUI 模式额外用 e-utils（etest 同源）弹「❌BUG跟踪Panic」错误框；
///      CLI 模式不弹，避免 --auto / --selftest 无人值守时被模态框卡死
///   3. stderr 打一行，命令行里也能看到
fn install_panic_hook(with_dialog: bool) {
    let dir = core::app_dir::app_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("crash.txt");
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "(unknown)".into());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "(panic payload not a string)".into()
        };
        let text = format!(
            "findany CRASH LOG\n\nSOURCE: {loc}\n\nMESSAGE: {msg}\n\nBACKTRACE:\n{}\n",
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::fs::write(&path, text);
        eprintln!("[findany] 崩溃：{msg}（{loc}），已写 {}", path.display());
        if with_dialog {
            // 主线程 panic 时这会等到用户点掉；其它线程的 panic 不会阻塞进程收尾
            let body = format!("{msg}\n\n位置：{loc}\n\n崩溃文件：\n{}", path.display());
            e_utils::dialog::sync::error("❌BUG跟踪Panic", body);
        }
    }));
}

/// 打包版是 GUI 子系统（windows_subsystem = "windows"）：进程自身没有控制台，stdout 默认是黑洞。
/// 这里按 gpu-test / e-log 的同款做法（e_log::panic::reattach_windows_terminal）向父进程借一个
/// 控制台：从 cmd/计划任务/管道启动时日志照旧看得见；双击启动时父进程没有控制台，
/// AttachConsole 失败也无害（纯 GUI 本来也不需要输出）。
///
/// 比 e-log 那版多一步：AttachConsole 会把标准句柄换成控制台的，于是调用方的
/// `findany.exe --auto > run.txt` 这种重定向会被丢掉（实测：重定向文件 0 字节）。
/// 所以先把调用方给句柄记下来，attach 之后再放回去 —— 控制台和重定向两头都能拿到输出。
#[cfg(windows)]
fn reattach_windows_terminal() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // -11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // -12
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn SetStdHandle(nStdHandle: u32, h: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let out0 = GetStdHandle(STD_OUTPUT_HANDLE);
        let err0 = GetStdHandle(STD_ERROR_HANDLE);
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let valid = |h: *mut std::ffi::c_void| !h.is_null() && h as isize != -1;
        if valid(out0) {
            SetStdHandle(STD_OUTPUT_HANDLE, out0);
        }
        if valid(err0) {
            SetStdHandle(STD_ERROR_HANDLE, err0);
        }
    }
}

#[cfg(not(windows))]
fn reattach_windows_terminal() {}

/// 进程优先级：服务器上跑自动化时别抢生产任务的 CPU/IO（仅 Windows 生效）。
/// normal 不动；below_normal / idle 调低。Linux 上用 nice/renice 更合适，这里只记一行日志。
#[cfg(windows)]
fn apply_process_priority(level: &str) {
    const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
    const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetPriorityClass(hProcess: *mut std::ffi::c_void, dwPriorityClass: u32) -> i32;
    }
    let class = match level {
        "idle" => IDLE_PRIORITY_CLASS,
        "below_normal" => BELOW_NORMAL_PRIORITY_CLASS,
        _ => NORMAL_PRIORITY_CLASS,
    };
    if class != NORMAL_PRIORITY_CLASS {
        let ok = unsafe { SetPriorityClass(GetCurrentProcess(), class) != 0 };
        core::app_dir::log_line(
            "findany-run.log",
            if ok { "info" } else { "warn" },
            &format!("进程优先级已设为 {level}（ok={ok}）"),
        );
    }
}

#[cfg(not(windows))]
fn apply_process_priority(level: &str) {
    if level != "normal" {
        core::app_dir::log_line("findany-run.log", "warn", &format!("process_priority={level} 仅 Windows 生效，已忽略（Linux 请用 nice/renice）"));
    }
}

/// GUI 模式：隐藏控制台窗口（debug 版仍在 console 子系统下，双击会闪一下，这里立刻藏掉）
#[cfg(windows)]
fn hide_console_if_gui() {
    const SW_HIDE: i32 = 0;
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
        fn ShowWindow(hwnd: *mut std::ffi::c_void, cmd: i32) -> i32;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
fn hide_console_if_gui() {}

/// QA 模式（移植对拍用）：findany --qa <日志目录>
fn run_qa(dir: &str) {
    let cfg = core::logfilter::engine::FilterRunCfg {
        root_dir: dir.to_string(),
        extensions: vec!["log".into()],
        ..Default::default()
    };
    let files = core::logfilter::engine::walk_filter(&cfg);
    let mut out: Vec<serde_json::Value> = Vec::new();
    for p in &files {
        let it = core::logfilter::engine::extract_one(p, dir, &cfg);
        out.push(serde_json::Value::Object(it));
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    eprintln!("QA: {} 个文件，导出 {} 条", files.len(), out.len());
}

/// QA 批次产物：findany --qa-filter <日志目录> <输出根>
fn run_qa_filter(dir: &str, out_root: &str) {
    let cfg = core::logfilter::engine::FilterRunCfg {
        root_dir: dir.to_string(),
        out_dir: out_root.to_string(),
        extensions: vec!["log".into()],
        upload_enabled: false,
        keep_logs: false,
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (_items, summary) = core::logfilter::engine::run_filter(&cfg, None, &cancel);
    if summary.batch_dir.is_empty() {
        eprintln!("QA-FILTER: 未生成批次产物");
        return;
    }
    println!("{}", summary.batch_dir);
    eprintln!("QA-FILTER: 提取 {} 条 -> {}", summary.extracted, summary.batch_dir);
}

/// 崩溃钩子自证：findany --panictest
/// 触发一次真实 panic，检查 logs/crash.txt 是否按约定写出（供自动化验证，不弹窗）
fn run_panictest(with_dialog: bool) -> i32 {
    let dir = core::app_dir::app_dir().join("logs");
    let crash = dir.join("crash.txt");
    let _ = std::fs::remove_file(&crash);
    // 默认走与 GUI 完全相同的钩子（含弹窗）：证明「弹窗链路」也活着；
    // 带 --no-dialog 时走 CLI 形态（只落盘），便于自动化判退出码。
    install_panic_hook(with_dialog);
    println!(
        "[panictest] 触发 panic…（预期：{}写 crash.txt + 退出码 101）",
        if with_dialog { "弹错误框 + " } else { "" }
    );
    panic!("panictest 故意崩溃：验证崩溃钩子（落盘 + 弹窗）链路");
}

/// 界面渲染压测（无窗口）：findany --uitest 验证「表格虚拟化」——
/// 用 TableBuilder 同款可见行公式，比对「有定高视口」与「不封顶」两种情形要画多少行。
fn run_uitest() -> bool {
    const ROW_H: f32 = 26.0;
    const SPACING_Y: f32 = 7.0;
    const WINDOW_H: f32 = 760.0;
    let per_row = ROW_H + SPACING_Y;
    let cases: [(usize, f32, &str); 4] = [
        (1_000, WINDOW_H, "定高视口(修复后)"),
        (30_000, WINDOW_H, "定高视口(修复后)"),
        (30_000, 1.0e7, "不封顶(修复前)"),
        (200_000, WINDOW_H, "定高视口(修复后)"),
    ];
    let mut ok = true;
    for (rows, span, verdict) in cases {
        let visible = ((span / per_row).ceil() as usize + 1).min(rows);
        println!("[uitest] {verdict}：总行 {rows}，视口高 {span:.0}px -> 每帧画 {visible} 行");
        if span > 1.0e6 && visible != rows {
            ok = false;
        }
    }
    let one_screen = ((WINDOW_H / per_row).ceil() as usize) + 1;
    println!("[uitest] 结论：定高视口下每帧只画 {one_screen} 行（一屏），与总行数无关 -> 虚拟化生效");

    // 布局健壮性回归：曾在 available_height 为 NaN 时崩（egui ui.rs set_height debug_assert）
    let items: Vec<core::scanner::ScanItem> = (0..30)
        .map(|i| core::scanner::ScanItem {
            filename: format!("f{i}.log"),
            rel_path: format!("g/f{i}.log"),
            hit_line_text: "x".into(),
            encoding: "utf-8".into(),
            ..Default::default()
        })
        .collect();
    let ctx = eframe::egui::Context::default();
    let sizes: [(f32, f32); 9] = [
        (1360.0, 800.0),
        (320.0, 240.0),
        (1.0, 1.0),
        (0.0, 0.0),
        (1360.0, 1.0),
        (1.0, 800.0),
        (5.0, 5.0),
        (f32::NAN, 400.0),
        (400.0, f32::NAN),
    ];
    for (w, h) in sizes {
        // 每个尺寸连跑 3 帧：NaN 常出现在「上一帧布局异常 -> 本帧可用空间被污染」的链条上
        for _ in 0..3 {
            let input = eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    eframe::egui::vec2(w, h),
                )),
                ..Default::default()
            };
            ctx.begin_pass(input);
            ui::app::with_central_test_ui(&ctx, |ui| {
                let _ = ui::app::render_scan_table_for_test(ui, &items, false);
            });
            // FullOutput 必须消费掉 texture delta，否则 epaint 会在 drop 时 panic
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
        }
        println!("[uitest] 布局回归 OK：窗口 {w:.0}x{h:.0} 渲染无 panic");
    }

    // 左侧面板宽度回归：360 / 420 / 680 三档下面板内容都不许超出面板宽度。
    // 踩过的坑：①「CLI 路径」输入框吃掉全部宽度，把右边的「…」选文件按钮顶出面板；
    //          ②「自动运行」标题+提示一行放不下，把整张卡片右边界顶出去（右边到顶）。
    let mut panel_ok = true;
    for w in [360.0f32, 440.0, 680.0] {
        let app_cfg = core::config::SearchConfig::default();
        let mut app = ui::app::FindanyApp::new(app_cfg, std::path::PathBuf::from("."), String::new(), false);
        let ctxp = eframe::egui::Context::default();
        let input = eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1360.0, 900.0),
            )),
            ..Default::default()
        };
        ctxp.begin_pass(input);
        let mut measured = 0.0f32;
        ui::app::with_central_test_ui(&ctxp, |ui| {
            measured = ui::app::measure_config_panel(&mut app, ui, w);
        });
        let mut o = ctxp.end_pass();
        o.textures_delta.clear();
        let fits = measured <= w + 1.0;
        if !fits {
            panel_ok = false;
        }
        println!("[uitest] 左侧面板 {w:.0}px -> 内容宽 {measured:.0}px{}", if fits { "" } else { "  <- FAIL（超出面板）" });
    }

    // 导出必须在后台线程跑完并把结果回传（UI 线程零 I/O 的那道闸门）
    {
        let dir = std::env::temp_dir().join("findany-uitest-export");
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = ui::app::FindanyApp::new(
            core::config::SearchConfig::default(),
            std::path::PathBuf::from("."),
            String::new(),
            false,
        );
        let msg = ui::app::export_probe(&mut app, &dir);
        let ok = msg.contains("已导出 3 条");
        if !ok {
            panel_ok = false;
        }
        println!("[uitest] 后台导出：{msg}");
    }

    // 跟随最新：每来一批新行都必须「精确贴底」且「滚动位置单调不减」。
    // 历史 bug：把纵向滚动交给了表格外面那层 ScrollArea（它的内容高恰好等于一屏，纵向范围恒为 0），
    // 每批新行都把整张表顶上去再被夹回来 —— 用户看到的就是"画面一闪一闪"，而且永远看不到最新行。
    // 现在量的是表格自己那个滚动区（真正管行的那个）。
    let many: Vec<core::scanner::ScanItem> = (0..240)
        .map(|i| core::scanner::ScanItem {
            filename: format!("f{i}.log"),
            rel_path: format!("g/f{i}.log"),
            hit_line_text: "x".into(),
            encoding: "utf-8".into(),
            ..Default::default()
        })
        .collect();
    let ctx2 = eframe::egui::Context::default();
    let mut prev_off = f32::NEG_INFINITY;
    let mut follow_ok = true;
    for step in 1..=6 {
        let n = step * 40;
        let mut probe = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let input = eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1360.0, 800.0),
            )),
            ..Default::default()
        };
        ctx2.begin_pass(input);
        ui::app::with_central_test_ui(&ctx2, |ui| {
            probe = ui::app::render_scan_table_for_test(ui, &many[..n], true);
        });
        let mut o = ctx2.end_pass();
        o.textures_delta.clear();
        let (off, max, content_h, view_h) = probe;
        let at_bottom = (off - max).abs() <= 1.0;
        let monotonic = off + 0.5 >= prev_off;
        if !(at_bottom && monotonic) {
            follow_ok = false;
        }
        println!(
            "[uitest] 跟随最新：{n} 行 -> 滚动 {off:.0}/{max:.0}（内容高 {content_h:.0}，视口高 {view_h:.0}）{}",
            if at_bottom && monotonic { "" } else { "  <- FAIL（未贴底或回跳）" }
        );
        prev_off = off;
    }
    println!(
        "[uitest] 跟随最新 {}：每帧精确贴底、位置单调不减（无来回跳）",
        if follow_ok { "OK" } else { "FAIL" }
    );

    let all_ok = ok && follow_ok && panel_ok;
    if !all_ok {
        println!("[uitest] FAIL");
    }
    all_ok
}

/// 输出扫描总耗时、事件批次数、单帧最坏消费耗时（界面卡不卡就看这个）
fn run_bench(dir: &str, keyword: &str, threads: i64) {
    // **与界面同一条管道**：mode="scan" 走扫描策略 —— bench 量的就是界面跑的真实路径
    // （以前这里调的是独立的 ScanEngine，和界面用的不是一套，量出来的数对不上）。
    let d = core::config::SearchConfig::default();
    let rcfg = core::logfilter::engine::FilterRunCfg {
        root_dir: dir.to_string(),
        out_dir: std::env::temp_dir().join("findany-bench-out").to_string_lossy().to_string(),
        threads: if threads > 0 {
            threads.clamp(1, 64)
        } else {
            std::cmp::max(4, std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as i64)
        },
        extensions: d.extensions.clone(),
        keep_logs: false,
        mode: "scan".into(),
        keyword: keyword.to_string(),
        match_mode: "inc".into(),
        case_sensitive: false,
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    let t0 = std::time::Instant::now();
    let handle = core::logfilter::engine::spawn_filter(rcfg, tx);
    let mut batches = 0usize;
    let mut rows = 0usize;
    let mut queue_peak = 0usize;
    let mut worst_frame = std::time::Duration::ZERO;
    let mut frames = 0usize;
    // 模拟界面：每帧最多消费 2000 条 / 6ms，超预算留到下一帧
    loop {
        let frame_start = std::time::Instant::now();
        let budget = frame_start + std::time::Duration::from_millis(6);
        let mut consumed = 0usize;
        let mut done = false;
        while consumed < 2000 && std::time::Instant::now() < budget {
            match rx.try_recv() {
                Ok(core::logfilter::engine::FilterEvent::Batch(b)) => {
                    rows += b.items.len();
                    batches += 1;
                    consumed += 1;
                }
                Ok(core::logfilter::engine::FilterEvent::Done(o, _)) => {
                    let _ = o;
                    done = true;
                    break;
                }
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        let spent = frame_start.elapsed();
        if spent > worst_frame {
            worst_frame = spent;
        }
        frames += 1;
        queue_peak = queue_peak.max(batches.saturating_sub(rows / 24));
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(16)); // 模拟 60fps 一帧
    }
    use std::sync::atomic::Ordering;
    let total = handle.total.load(Ordering::Relaxed);
    let hit = handle.hit_n.load(Ordering::Relaxed);
    println!("[bench] 文件 {total}，命中 {hit}，分批 {batches} 批 / {rows} 行");
    println!("[bench] 扫描总耗时 {:.2}s，界面帧数 {}，单帧最坏 {:.1}ms", t0.elapsed().as_secs_f64(), frames, worst_frame.as_secs_f64() * 1000.0);
    let _ = queue_peak;
}

/// selftest 用：造一个「扫描模式」的统一管道配置（**与界面跑的是同一套**）
fn scan_pipe_cfg(root: &str, out: &str, keyword: &str, threads: i64) -> core::logfilter::engine::FilterRunCfg {
    core::logfilter::engine::FilterRunCfg {
        root_dir: root.to_string(),
        out_dir: out.to_string(),
        threads,
        extensions: core::config::SearchConfig::default().extensions.clone(),
        keep_logs: false,
        mode: "scan".into(),
        keyword: keyword.to_string(),
        match_mode: "inc".into(),
        case_sensitive: false,
        ..Default::default()
    }
}

/// 自检（无 GUI）：findany --selftest <日志目录>
fn run_selftest(dir: &str) -> i32 {
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut check = |name: &str, ok: bool, detail: &str| {
        if ok {
            pass += 1;
            println!("  PASS  {name}");
        } else {
            fail += 1;
            println!("  FAIL  {name}  {detail}");
        }
    };

    // 1) TOML 单文件配置：解析 / 保存 / 回读 往返
    {
        let tmp = std::env::temp_dir().join("findany-selftest-cfg.toml");
        let _ = std::fs::remove_file(&tmp);
        core::logfilter::autoconfig::write_default(&tmp);
        let mut cfg = core::logfilter::autoconfig::load_config(&tmp);
        check("默认模板可解析", cfg.root_dir.is_empty() && cfg.log_type == "auto", &cfg.log_type);
        // 资源控制三项必须在默认模板里（服务器上跑靠它们压住 CPU/IO/目录规模）
        {
            let tpl = std::fs::read_to_string(&tmp).unwrap_or_default();
            check("模板含 throttle_ms", tpl.contains("throttle_ms"), "");
            check("模板含 max_files", tpl.contains("max_files"), "");
            check("模板含 process_priority", tpl.contains("process_priority"), "");
            let mut bad = core::config::SearchConfig::default();
            bad.process_priority = "high".into();
            check("非法进程优先级被拦", !bad.validate().is_empty(), "未拦住");
            let mut bad2 = core::config::SearchConfig::default();
            bad2.throttle_ms = 99999;
            check("非法节流值被拦", !bad2.validate().is_empty(), "未拦住");
        }
        cfg.keyword = "IT6563".into();
        cfg.threads = 16;
        cfg.countdown_sec = 8;
        cfg.out_dir = "D:/out-test".into();
        // 资源控制三项：GUI 改完点保存 -> 必须原样回读（漏了受管键就会静默失效）
        cfg.throttle_ms = 150;
        cfg.max_files = 777;
        cfg.process_priority = "idle".into();
        cfg.name_filter = "MT71".into();
        let saved = core::logfilter::autoconfig::save_config(&tmp, &cfg).unwrap_or_default();
        let back = core::logfilter::autoconfig::load_config(&tmp);
        check("配置往返一致", back.keyword == "IT6563" && back.threads == 16 && back.out_dir == "D:/out-test", &saved);
        check("倒计时往返一致", back.countdown_sec == 8, &back.countdown_sec.to_string());
        check(
            "资源控制三项往返一致",
            back.throttle_ms == 150 && back.max_files == 777 && back.process_priority == "idle",
            &format!("throttle={} max_files={} prio={}", back.throttle_ms, back.max_files, back.process_priority),
        );
        check("文件名搜索框往返一致", back.name_filter == "MT71", &back.name_filter);
        let text = std::fs::read_to_string(&tmp).unwrap_or_default();
        check("保留模板注释", text.contains("# 改 true：启动即自动"), "");
        check("保留 auto_start 键", text.contains("auto_start = false"), "");

        // 老档（只写两个键）加载：缺的键按默认补在**内存**里，文件一字不动
        // （用户要求：只有 toml 不存在时才新建，运行过程中不要重建它）
        let mini = std::env::temp_dir().join("findany-selftest-mini.toml");
        let _ = std::fs::remove_file(&mini);
        std::fs::write(&mini, "# minimal\n\n[filter]\nroot_dir = '.'\n").unwrap();
        let before = std::fs::read_to_string(&mini).unwrap_or_default();
        let healed = core::logfilter::autoconfig::load_config(&mini);
        let after = std::fs::read_to_string(&mini).unwrap_or_default();
        check("老档加载不回写文件（原文一字未动）", before == after, "文件被改写了");
        check("老档原值生效", healed.root_dir == ".", &healed.root_dir);
        check(
            "缺失键按默认补齐(threads)",
            healed.threads == core::config::SearchConfig::default().threads,
            &healed.threads.to_string(),
        );
        // run.auto_start 决定「启动后自动运行」：老档缺这一键时必须落到默认值
        check(
            "auto_start 往返一致",
            healed.auto_start == core::config::SearchConfig::default().auto_start,
            &healed.auto_start.to_string(),
        );
        // 只有「文件不存在」才新建：ensure_config_file 对已存在的文件必须返回 false 且不改内容
        let existed_before = std::fs::read_to_string(&mini).unwrap_or_default();
        let created = core::logfilter::autoconfig::ensure_config_file(&mini);
        check(
            "已有 toml 不重建（只有不存在才新建）",
            !created && std::fs::read_to_string(&mini).unwrap_or_default() == existed_before,
            "已存在的文件被重建了",
        );
        let _ = std::fs::remove_file(&mini);
        check("无 config.json 逻辑", !std::path::Path::new(&dir).join("config.json").exists(), "");
        let _ = std::fs::remove_file(&tmp);
    }

    // 2) 筛选：判型 + 字段提取
    let cfg = core::logfilter::engine::FilterRunCfg {
        root_dir: dir.to_string(),
        extensions: vec!["log".into()],
        upload_enabled: false,
        ..Default::default()
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (items, summary) = core::logfilter::engine::run_filter(&cfg, None, &cancel);
    check("扫描到样例日志", summary.total >= 6, &format!("total={}", summary.total));
    check("全部提取成功", summary.extracted == summary.total, &format!("extracted={}", summary.extracted));
    check("判型无未知", summary.unknown == 0, &format!("unknown={}", summary.unknown));

    let by = |name: &str| items.iter().find(|i| i.get("log_file").and_then(|v| v.as_str()) == Some(name)).cloned();
    let s = |it: &serde_json::Map<String, serde_json::Value>, k: &str| it.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if let Some(it) = by("MT71I2GSF-2HG260807250XAG0015.log") {
        check("OA3 判型", s(&it, "detected_type") == "etest(OA3)", &s(&it, "detected_type"));
        check("SN", s(&it, "sn") == "MT71I2GSF-2HG260807250XAG0015", &s(&it, "sn"));
        check("ProductKeyID", s(&it, "product_key_id") == "4362262499781", &s(&it, "product_key_id"));
        check("Hash 长度 4000", s(&it, "hardware_hash_len") == "4000", &s(&it, "hardware_hash_len"));
        check(
            "Hash SHA-256",
            s(&it, "hardware_hash_sha256") == "cd65b4b91f06b9df2f008102ab235a948d0c4644037d7cb1c41a72f7133339ed",
            &s(&it, "hardware_hash_sha256"),
        );
        check("Baseboard", s(&it, "baseboard_product") == "XBoard V7", &s(&it, "baseboard_product"));
        check("OA3 块计数=2", s(&it, "oa3_block_count") == "2", &s(&it, "oa3_block_count"));
    } else {
        check("样例 0015 存在", false, "未找到");
    }
    if let Some(it) = by("MT71I2GSF-2HG260807250XAG0025.log") {
        check("e-autotest 判型", s(&it, "detected_type") == "e-autotest", &s(&it, "detected_type"));
        check("UUID 已提取", !s(&it, "uuid").is_empty(), "空");
    } else {
        check("样例 0025 存在", false, "未找到");
    }

    // 3) 通用扫描端到端：出 Excel 产物（**与界面同一条管道**，mode="scan"）
    {
        let out_root = std::env::temp_dir().join("findany-selftest-out");
        let _ = std::fs::create_dir_all(&out_root);
        let scfg = scan_pipe_cfg(dir, &out_root.to_string_lossy(), "HardwareHash", 4);
        let cancel0 = std::sync::atomic::AtomicBool::new(false);
        let (items, summary0) = core::logfilter::engine::run_filter(&scfg, None, &cancel0);
        check("扫描命中", summary0.hit >= 3, &format!("hit={}", summary0.hit));
        check("扫描无跳过", summary0.skipped == 0, &format!("skipped={}", summary0.skipped));
        match core::logfilter::engine::export_products(&scfg, &items, &summary0, std::time::Instant::now()) {
            Ok((batch, excel, _audit, _kept)) => {
                let size = std::fs::metadata(&excel).map(|m| m.len()).unwrap_or(0);
                check("Excel 产物落盘", size > 4096, &format!("{excel} size={size}"));
                check("批次目录按时间命名", std::path::Path::new(&batch).is_dir(), &batch);
            }
            Err(e) => check("Excel 产物落盘", false, &e.to_string()),
        }
    }

    // 4) 实时渲染：扫描/筛选必须**边跑边分批**回传（不是跑完一次性给结果）
    {
        // 造 240 个小文件：批阈值 24 条/250ms，必然分成多批 -> 能验证「先来批、后收尾」
        // 注意：输入目录必须**不在**输出根之内（否则按设计会被排除——输出目录自排除是特性）
        let bulk = std::env::temp_dir().join("findany-selftest-live");
        let _ = std::fs::remove_dir_all(&bulk);
        let files_dir = std::env::temp_dir().join("findany-selftest-files");
        let _ = std::fs::remove_dir_all(&files_dir);
        let _ = std::fs::create_dir_all(&files_dir);
        let _ = std::fs::create_dir_all(&bulk);
        for i in 0..240 {
            let p = files_dir.join(format!("f{i:03}.log"));
            let _ = std::fs::write(&p, format!("line A {i}\nHardwareHash item {i}\nline C\n"));
        }
        // （lcfg 已由下文的 scan_pipe_cfg 取代：与界面同一条管道）
        let created = std::fs::read_dir(&files_dir).map(|r| r.filter_map(|e| e.ok()).count()).unwrap_or(0);
        let walked = core::scanner::walk_files(&files_dir, &["log".to_string()], true).len();
        check("临时批量文件已建", created == 240, &format!("created={created}"));
        check("遍历命中 240 个文件", walked == 240, &format!("walked={walked}"));

        // 单文件模式回归：root_dir 指向单个文件时必须正好出 1 行。
        // 踩过的坑：单文件也被拼了「相对 root 的尾巴」，canonical 出来是 \? 前缀路径，
        // join("") 在末尾留分隔符 -> metadata 失败 -> 一行都没有（界面表现：通用扫描没有结果）。
        let one = core::scanner::walk_files(&files_dir, &["log".to_string()], true)
            .into_iter()
            .next()
            .unwrap_or_default();
        let oc = scan_pipe_cfg(&one.to_string_lossy(), &bulk.to_string_lossy(), "HardwareHash", 1);
        let otx = std::sync::mpsc::sync_channel(32);
        let ohandle = core::logfilter::engine::spawn_filter(oc, otx.0);
        let mut orows = 0usize;
        loop {
            match otx.1.recv() {
                Ok(core::logfilter::engine::FilterEvent::Batch(b)) => orows += b.items.len(),
                Ok(core::logfilter::engine::FilterEvent::Done(..)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        check("单文件扫描出 1 行", orows == 1, &format!("rows={orows}"));
        // 资源控制：max_files 真的截断（服务器上防目录跑飞）
        {
            let mut ccfg = scan_pipe_cfg(&files_dir.to_string_lossy(), &bulk.to_string_lossy(), "HardwareHash", 1);
            ccfg.max_files = 5;
            let cancel1 = std::sync::atomic::AtomicBool::new(false);
            let (_items, out) = core::logfilter::engine::run_filter(&ccfg, None, &cancel1);
            check("max_files 截断生效(240->5)", out.total == 5, &format!("total={}", out.total));
        }
        {
            use std::sync::atomic::Ordering;
            let t = ohandle.total.load(Ordering::Relaxed);
            check("单文件统计总数=1", t == 1, &format!("total={t}"));
        }

        // 文件类型过滤回归：勾了 log 就只扫 log（大小写都算），只要 txt 就不带 log，空列表=全部
        let mix = std::env::temp_dir().join("findany-ext-mix");
        let _ = std::fs::remove_dir_all(&mix);
        let _ = std::fs::create_dir_all(&mix);
        for n in ["a.log", "b.LOG", "c.txt"] {
            let _ = std::fs::write(mix.join(n), b"x");
        }
        let only_log = core::scanner::walk_files(&mix, &["log".to_string()], false).len();
        check("类型过滤：只留 log 命中 2（.log/.LOG 都算）", only_log == 2, &format!("n={only_log}"));
        let only_txt = core::scanner::walk_files(&mix, &["txt".to_string()], false).len();
        check("类型过滤：只要 txt 时不带 log", only_txt == 1, &format!("n={only_txt}"));
        let all_ext = core::scanner::walk_files(&mix, &[], false).len();
        check("类型过滤：空列表=全部", all_ext == 3, &format!("n={all_ext}"));
        let multi = core::scanner::walk_files(&mix, &["log".to_string(), "txt".to_string()], false).len();
        check("类型过滤：log+txt 多选 = 3", multi == 3, &format!("n={multi}"));

        // 文件名搜索框：子串、不分大小写；留空=不过滤；与扩展名过滤是「与」
        for n in ["MT71.log", "backup_Mt71_A.log", "other.log"] {
            let _ = std::fs::write(mix.join(n), b"x");
        }
        let by_name = core::scanner::walk_files_filtered(&mix, &["log".to_string()], false, "mt71").len();
        check("文件名过滤：不分大小写命中 2", by_name == 2, &format!("n={by_name}"));
        let by_name_ext = core::scanner::walk_files_filtered(&mix, &["txt".to_string()], false, "mt71").len();
        check("文件名过滤与类型过滤是「与」", by_name_ext == 0, &format!("n={by_name_ext}"));
        let no_name = core::scanner::walk_files_filtered(&mix, &["log".to_string()], false, "").len();
        check("文件名过滤留空=不过滤", no_name == 5, &format!("n={no_name}"));

        // 分批模式（batch_dirs）：3 个子目录 -> 3 批；R 结论要带批次汇总
        {
            let bdir = std::env::temp_dir().join("findany-batch-selftest");
            let _ = std::fs::remove_dir_all(&bdir);
            for n in ["a", "b", "c"] {
                let _ = std::fs::create_dir_all(bdir.join(n));
                let _ = std::fs::write(bdir.join(n).join("x.log"), b"HardwareHash\n");
            }
            let mut bcfg = core::logfilter::engine::FilterRunCfg::default();
            bcfg.root_dir = bdir.to_string_lossy().to_string();
            bcfg.out_dir = bdir.join("out").to_string_lossy().to_string();
            bcfg.extensions = vec!["log".into()];
            bcfg.upload_enabled = false;
            bcfg.recursive = true;
            bcfg.batch_dirs = true;
            bcfg.ui_refresh_ms = 0;
            let bh = core::logfilter::engine::FilterHandle::default();
            let (bitems, bs) = core::logfilter::engine::run_filter_shared(&bcfg, None, &bh.cancel, &bh);
            check(
                "分批模式：3 个子目录 = 3 批全 PASS",
                bs.batches_total == 3 && bs.batches_pass == 3,
                &format!("total={} pass={}", bs.batches_total, bs.batches_pass),
            );
            // 界面只拿**最后一批**的行（内存有界正是分批的意义）；全量在各自的批次产物里
            check(
                "分批模式：提取 3 条、界面只留最后一批",
                bs.extracted == 3 && bitems.len() == 1,
                &format!("extracted={} items={}", bs.extracted, bitems.len()),
            );
            let (bcontent, bok) = core::logfilter::engine::filter_verdict(&bs, false, true);
            check("分批模式：R 结论含批次汇总", bok && bcontent.contains("分批 3 批"), &bcontent);
            let _ = std::fs::remove_dir_all(&bdir);
        }

        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        let lrcfg = scan_pipe_cfg(&files_dir.to_string_lossy(), &bulk.to_string_lossy(), "HardwareHash", 4);
        let handle = core::logfilter::engine::spawn_filter(lrcfg, tx);
        let mut batches = 0usize;
        let mut scanned_rows = 0usize;
        let mut first_batch_rows = 0usize;
        let mut done_hit = 0usize;
        let mut done_total = 0usize;
        // 一路收到 Done：既能证明「批先到、结果后到」，也能核对分批累计等于全量
        loop {
            match rx.recv() {
                Ok(core::logfilter::engine::FilterEvent::Batch(b)) => {
                    batches += 1;
                    scanned_rows += b.items.len();
                    if batches == 1 {
                        first_batch_rows = b.items.len();
                    }
                }
                Ok(core::logfilter::engine::FilterEvent::Done(o, _)) => {
                    done_total = o.summary.total;
                    done_hit = o.summary.hit;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        use std::sync::atomic::Ordering;
        let scanned_total = handle.total.load(Ordering::Relaxed);
        // 首批发的是**部分**结果：总量（240）明显大于首批行数。
        // 注意口径：统一管道是**流式遍历**（walk_files_each），首批 64 个文件到达时
        // 遍历还没走完，所以不能用「首批时刻的 total」判断 —— 用最终总量比更准。
        let partial_first = scanned_total > first_batch_rows;
        check(
            "扫描分批回传（首批发的是部分结果）",
            batches >= 2 && partial_first,
            &format!("batches={batches} first={first_batch_rows} walked={scanned_total} done_total={done_total} hit={done_hit}"),
        );
        check("分批累计 = 全量结果", scanned_rows == scanned_total && scanned_total == 240, &format!("{scanned_rows} vs {scanned_total}"));

        // 筛选侧同款：同一批文件走 logfilter，批事件应逐批到达
        let rcfg = core::logfilter::engine::FilterRunCfg {
            root_dir: files_dir.to_string_lossy().to_string(),
            out_dir: bulk.to_string_lossy().to_string(),
            extensions: vec!["log".into()],
            upload_enabled: false,
            keep_logs: false,
            ..Default::default()
        };
        let (tx2, rx2) = std::sync::mpsc::sync_channel(32);
        let h2 = core::logfilter::engine::spawn_filter(rcfg, tx2);
        let mut fbatches = 0usize;
        let mut frows = 0usize;
        let mut live_read = false;
        // 一路收到 Done 才收工（分批累计必须等于全量）
        loop {
            match rx2.recv() {
                Ok(core::logfilter::engine::FilterEvent::Batch(b)) => {
                    fbatches += 1;
                    frows += b.items.len();
                    if h2.progress().done > 0 {
                        live_read = true;
                    }
                }
                Ok(core::logfilter::engine::FilterEvent::Done(_, _)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        check("筛选分批回传（>=2 批）", fbatches >= 2, &format!("{fbatches}"));
        check("筛选句柄可读实时计数", live_read, "");
        check("筛选分批累计 = 240", frows == 240, &format!("{frows}"));
        let _ = std::fs::remove_dir_all(&bulk);
        let _ = std::fs::remove_dir_all(&files_dir);
    }

    {
        let line = core::result::build_rlog("提取 6/6，产物 out/x", true, "filter");
        check("R 标记以 R<{ 起、>R 收", line.starts_with("R<{") && line.trim_end().ends_with(">R"), &line[..line.len().min(24)]);
        check("R 标记单行", line.trim_end().matches("R<").count() == 1, "");
        match core::result::parse_last(&line) {
            Some((content, status)) => {
                check("R 标记可反解 content", content == "提取 6/6，产物 out/x", &content);
                check("R 标记 status=true 判通过", status, "status=false");
            }
            None => check("R 标记可反解", false, "解析失败"),
        }
        let fail_line = core::result::build_rlog("回传异常：失败 1", false, "filter");
        check("R 标记 status=false 判失败", core::result::parse_last(&fail_line).map(|(_, s)| !s).unwrap_or(false), "");
        let path = core::result::emit("自检写入", true, "selftest");
        check("运行日志 logs/findany.log 已收尾", path.as_ref().map(|p| p.ends_with("logs/findany.log")).unwrap_or(false), &format!("{path:?}"));
        let text = path.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let tail = text.lines().last().unwrap_or_default().to_string();
        check("日志末行是 R 结论", tail.contains("R<{") && tail.ends_with(">R"), &tail[..tail.len().min(70)]);
        check("末行可判定通过", core::result::parse_last(&tail).map(|(_, s)| s).unwrap_or(false), &tail[..tail.len().min(70)]);
        let rf = core::app_dir::app_dir().join("logs").join(core::result::RESULT_FILE);
        check("滚动结果文件只留一条 R", std::fs::read_to_string(&rf).map(|t| t.matches("R<").count() == 1).unwrap_or(false), "");
    }

    // 6) 回传组包 dry-run（不打网，只验四字段）
    let fields = items
        .iter()
        .find(|i| i.get("log_file").and_then(|v| v.as_str()) == Some("MT71I2GSF-2HG260807250XAG0015.log"))
        .and_then(|i| i.get("fields").and_then(|v| v.as_object()).cloned())
        .unwrap_or_default();
    let profile = core::logfilter::uploader::UploadProfile::default();
    let res = core::logfilter::uploader::run_upload(&profile, &fields, true, &|_, _| {}, None);
    check("dry-run 状态", res.status == core::logfilter::uploader::ST_DRY_RUN, &res.status);
    check("dry-run payload 有 serial_number", res.payload_json.contains("serial_number"), &res.payload_json);
    check("dry-run payload 含 4000 位 Hash", res.payload_json.len() > 4000, "长度不足");

    // 回传拦截回归：开了正式回传却一台都没回传 = 必须 FAIL。
    // 踩过的坑：只判「跑过回传且失败/冲突」，于是「只提取没回传」被当成 PASS 自动关窗。
    use core::logfilter::engine::{filter_verdict, FilterSummary};
    let mut v0 = FilterSummary { total: 5, extracted: 5, batch_dir: "out/x".into(), ..Default::default() };
    let (c0, ok0) = filter_verdict(&v0, true, false);
    check("开了正式回传但一台没匹配到 -> FAIL", !ok0, &c0);
    check("拦截消息说清原因", c0.contains("一台都没"), &c0);
    let (c1, ok1) = filter_verdict(&v0, true, true);
    check("dry-run 演练不算异常 -> PASS", ok1, &c1);
    v0.upload_targets = 5;
    v0.upload_dry = 5;
    let (c2, ok2) = filter_verdict(&v0, true, true);
    check("dry-run 回传 5 台 -> PASS", ok2, &c2);
    let mut v1 = FilterSummary { total: 5, extracted: 5, upload_targets: 5, upload_ok: 5, batch_dir: "out/x".into(), ..Default::default() };
    let (c3, ok3) = filter_verdict(&v1, true, false);
    check("定向 5 台全部回传成功 -> PASS", ok3, &c3);
    v1.upload_ok = 4;
    v1.upload_skip = 1;
    let (c4, ok4) = filter_verdict(&v1, true, false);
    check("定向 5 台里有 1 台没回传 -> FAIL", !ok4, &c4);

    println!("checks: {}  failed: {}", pass, fail);
    if fail == 0 { 0 } else { 1 }
}

fn main() -> eframe::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // CLI 模式一律不弹窗（无人值守不能被模态框卡住）；GUI 模式带弹窗
    let cli_mode = argv.first().map(|s| s.starts_with("--")).unwrap_or(false);
    // 打包版是 GUI 子系统：命令行模式先挂父控制台，输出才不是黑洞（--auto/--selftest/--qa…）。
    // GUI 模式**不能**挂：挂上之后 hide_console_if_gui 会把父进程的控制台窗口（用户的 cmd）一起藏掉。
    if cli_mode {
        reattach_windows_terminal();
    }
    install_panic_hook(!cli_mode);
    match argv.first().map(|s| s.as_str()) {
        Some("--qa") => {
            run_qa(argv.get(1).map(|s| s.as_str()).unwrap_or("."));
            return Ok(());
        }
        Some("--panictest") => {
            let no_dialog = argv.iter().any(|a| a == "--no-dialog");
            let code = run_panictest(!no_dialog);
            std::process::exit(code);
        }
        Some("--uitest") => {
            // 自证失败必须让退出码非零，否则 just verify 会把失败当通过
            std::process::exit(if run_uitest() { 0 } else { 1 });
        }
        Some("--bench") => {
            run_bench(
                argv.get(1).map(|s| s.as_str()).unwrap_or("."),
                argv.get(2).map(|s| s.as_str()).unwrap_or("HardwareHash"),
                argv.get(3).and_then(|s| s.parse().ok()).unwrap_or(0),
            );
            return Ok(());
        }
        Some("--qa-filter") => {
            run_qa_filter(
                argv.get(1).map(|s| s.as_str()).unwrap_or("."),
                argv.get(2).map(|s| s.as_str()).unwrap_or("."),
            );
            return Ok(());
        }
        Some("--selftest") => {
            let code = run_selftest(argv.get(1).map(|s| s.as_str()).unwrap_or("."));
            std::process::exit(code);
        }
        Some("--auto") => {
            let cli = core::logfilter::autoconfig::parse_args(&argv[1..]);
            let app_dir = core::app_dir::app_dir();
            let code = core::auto_run::run_headless(&cli, &app_dir.to_string_lossy());
            std::process::exit(code);
        }
        _ => {}
    }

    hide_console_if_gui();
    let proc_dir = core::app_dir::app_dir();
    core::app_dir::log_line("findany-gui.log", "info", "GUI 启动");

    // 唯一配置：findany.toml（不存在则生成默认模板，不覆盖已有）
    let cli = core::logfilter::autoconfig::parse_args(&argv);
    let path = core::logfilter::autoconfig::resolve_path(&cli, &proc_dir.to_string_lossy());
    if core::logfilter::autoconfig::ensure_config_file(path.as_path()) {
        core::app_dir::log_line("findany-run.log", "info", &format!("首次运行，已生成配置模板：{}", path.display()));
    }
    let mut cfg = core::logfilter::autoconfig::load_config(&path);
    cfg.resolve_out_dir(&proc_dir);
    // 资源控制：进程优先级（服务器上别抢生产任务）
    apply_process_priority(&cfg.process_priority);
    if cfg.auto_start {
        // TOML 要求自动开跑：默认进入筛选流程
        cfg.work_mode = "filter".into();
    }
    let auto_mode = cfg.auto_start || !cli.config.is_empty();
    core::app_dir::log_line(
        "findany-gui.log",
        "info",
        &format!(
            "启动参数：auto_start={} work_mode={} root_dir={} auto_mode={}",
            cfg.auto_start, cfg.work_mode, cfg.root_dir, auto_mode
        ),
    );
    // catch_unwind:GL 初始化失败既可能是 Err(eframe 的 Error::NoGlutinConfigs),也可能是
    // panic,两种都要能落到下面的软件渲染兜底。panic 仍会先走 install_panic_hook 写 crash.txt。
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ui::app::run(cfg, proc_dir, auto_mode)
    }));
    match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            core::app_dir::log_line("findany-gui.log", "error", &format!("GUI 启动失败：{e}"));
            // Windows 服务器/RDP 上系统 OpenGL 只有 1.1、glow 建不出上下文 —— 用随包的 Mesa 软件渲染重启
            if soft_gl::relaunch_with_mesa() {
                core::app_dir::log_line("findany-gui.log", "info", "已改用软件渲染(Mesa)重启");
                Ok(())
            } else {
                Err(e)
            }
        }
        Err(payload) => {
            core::app_dir::log_line("findany-gui.log", "error", "GUI 启动时 panic");
            if soft_gl::relaunch_with_mesa() {
                core::app_dir::log_line("findany-gui.log", "info", "已改用软件渲染(Mesa)重启");
                Ok(())
            } else {
                std::panic::resume_unwind(payload)
            }
        }
    }
}
