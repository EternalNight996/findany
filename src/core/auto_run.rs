//! 自动化运行（对齐 Python 版 TOML 流程：检测 → 回传 → 倒计时关）。
//!
//! 入口两条：
//!   findany --auto [--config x.toml]   无窗口跑完即退（产线无人值守）
//!   findany.toml 里 run.auto_start=true  GUI 起来即自动开跑

use crate::core::config::SearchConfig;
use crate::core::logfilter::engine::{self as fengine, FilterRunCfg};
use crate::core::logfilter::uploader::UploadProfile;

use crate::core::logfilter::autoconfig::CliArgs;

/// 由 SearchConfig 组装筛选配置（GUI 与无窗口模式共用）
pub fn filter_cfg_from_cfg(cfg: &SearchConfig, app_dir: &str) -> FilterRunCfg {
    let mut rcfg = FilterRunCfg {
        root_dir: cfg.root_dir.clone(),
        out_dir: cfg.out_dir.clone(),
        log_type: cfg.log_type.clone(),
        recursive: cfg.recursive,
        extensions: cfg.extensions.clone(),
        encoding: cfg.encoding.clone(),
        max_file_mb: cfg.max_file_mb,
        threads: cfg.threads,
        keep_logs: cfg.copy_files,
        upload_enabled: cfg.enabled,
        upload_types: cfg.types.clone(),
        dry_run: cfg.dry_run,
        profile: UploadProfile {
            // CLI 路径在这里就解析好（显式路径 > 程序目录 > doc/devicehashupload）：
            // 筛选回传、历史重传、导出、无窗口模式全都走这一处，避免某条路忘了兜底
            // —— 历史重传曾经正因为没解析，一开跑就是「CLI 不存在」，整批全失败。
            cli_path: crate::core::logfilter::uploader::resolve_cli(&cfg.cli_path, app_dir),
            args: cfg.args.clone(),
            use_stdin: cfg.use_stdin(),
            timeout_sec: cfg.timeout_sec,
            max_retries: cfg.max_retries,
            secret_key: cfg.secret_key.clone(),
            ..Default::default()
        },
        app_dir: app_dir.to_string(),
        ui_refresh_ms: cfg.ui_refresh_ms,
        name_filter: cfg.name_filter.clone(),
        mem_limit_mb: cfg.mem_limit_mb,
        batch_dirs: cfg.batch_dirs,
        batch_name_filter: cfg.batch_name_filter.clone(),
        throttle_ms: cfg.throttle_ms,
        max_files: cfg.max_files,
        cache_capacity_rows: cfg.cache_capacity_rows,
        // 统一管道的策略选择：TOML 的 work_mode 决定跑扫描/筛选/重传（差异只在这里）。
        // retry 必须原样透传：run_headless 靠它分流去「历史结果重传」，落成 filter 会静默变成日志筛选。
        mode: match cfg.work_mode.as_str() {
            "scan" | "retry" => cfg.work_mode.clone(),
            _ => "filter".into(),
        },
        keyword: cfg.keyword.clone(),
        match_mode: cfg.mode.clone(),
        case_sensitive: cfg.case_sensitive,
        // 断点续扫：进度文件路径稍后按任务指纹补（无窗口模式也落 —— 中断后能续）
        progress_path: String::new(),
        skip_dirs: Default::default(),
    };
    // 指纹依赖 root/mode/扩展名/名过滤，上面都填好了，这里补进度文件路径
    let fp = crate::core::logfilter::resume::task_fingerprint(&rcfg);
    rcfg.progress_path = crate::core::logfilter::resume::progress_path(&rcfg.out_dir, &fp);
    rcfg
}

/// 无窗口自动化：跑完返回退出码（0=成功，1=有失败项，2=首次生成模板）
pub fn run_headless(cli: &CliArgs, app_dir: &str) -> i32 {
    let path = crate::core::logfilter::autoconfig::resolve_path(cli, app_dir);
    let mut cfg;
    if path.is_file() {
        // 统一走 load_config：解析 + 老档自愈回写（与 GUI 同一套规则）
        cfg = crate::core::logfilter::autoconfig::load_config(&path);
    } else {
        {
            // 与 Python 一致：找不到 toml 就输出一份默认模板并提示，而不是直接报错
            let created = crate::core::logfilter::autoconfig::write_default(&path);
            crate::core::app_dir::log_line(
                "findany-run.log",
                "info",
                &format!("未找到 {}，已生成默认模板（created={created}）", path.display()),
            );
            println!("[findany] 未找到 {}，已生成默认模板", path.display());
            println!("[findany] 编辑 root_dir 与 upload.* 后重新运行 --auto");
            return 2;
        }
    }
    cfg.resolve_out_dir(std::path::Path::new(app_dir));
    // 资源控制：无窗口模式也要按配置调低进程优先级（服务器上别抢生产任务）
    crate::apply_process_priority(&cfg.process_priority);

    let rcfg = filter_cfg_from_cfg(&cfg, app_dir);
    crate::core::app_dir::log_line(
        "findany-run.log",
        "info",
        &format!("自动化开跑 root={} 类型={} 回传={}", rcfg.root_dir, rcfg.log_type, rcfg.upload_enabled),
    );
    let cancel = std::sync::atomic::AtomicBool::new(false);

    // 重传模式：跑的是「历史结果 xlsx 逐行重传」，不是日志管道。
    // findany.toml 是 GUI 与自动化共用的 —— GUI 停在重传模式保存后，产线的 --auto 必须也真的重传，
    // 否则会静默变成日志筛选（这是最危险的那种失败：看着跑完了，其实一台都没回传）。
    if rcfg.mode == "retry" {
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        let _handle = crate::core::logfilter::retry::spawn_retry(
            rcfg.root_dir.clone(),
            rcfg.out_dir.clone(),
            rcfg.profile.clone(),
            rcfg.dry_run,
            tx,
        );
        // 每个文件完成都会带一份 summary；正常以 Done 收尾，通道断了就用最后一份
        let mut summary = fengine::FilterSummary::default();
        loop {
            match rx.recv() {
                Ok(fengine::FilterEvent::Batch(out)) => summary = out.summary,
                Ok(fengine::FilterEvent::Done(out, _)) => {
                    summary = out.summary;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        crate::core::app_dir::log_line(
            "findany-run.log",
            "info",
            &format!(
                "自动化结束（重传）：结果文件 {}，回传 成功{}/冲突{}/失败{}，耗时 {}s",
                summary.extracted, summary.upload_ok, summary.upload_conflict, summary.upload_fail, summary.elapsed
            ),
        );
        let (content, ok) = crate::core::logfilter::retry::retry_verdict(&summary, rcfg.dry_run);
        crate::core::result::emit(&content, ok, "auto");
        if ok {
            println!("[findany] PASS {content}");
            0
        } else {
            println!("[findany] FAIL {content}");
            1
        }
    } else {
        let (_items, summary) = fengine::run_filter(&rcfg, None, &cancel);
        crate::core::app_dir::log_line(
            "findany-run.log",
            "info",
            &format!(
                "自动化结束：文件 {}，提取 {}，跳过 {}，回传 成功{}/冲突{}/失败{}，耗时 {}s，产物 {}",
                summary.total, summary.extracted, summary.skipped, summary.upload_ok, summary.upload_conflict, summary.upload_fail, summary.elapsed, summary.batch_dir
            ),
        );
        let (content, ok) = fengine::filter_verdict(&summary, rcfg.upload_enabled, rcfg.dry_run);
        crate::core::result::emit(&content, ok, "auto");
        if ok {
            println!("[findany] PASS {content}");
            0
        } else {
            println!("[findany] FAIL {content}");
            1
        }
    }
}
