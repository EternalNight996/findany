//! 内存看门狗：大目录（百万级文件）跑起来最怕的是**被系统挤爆直接消失**——
//! 那种死法没有 panic、没有弹窗、日志也停在半截。
//! 这里做两件事：
//!   1. 周期性把「进度 + 当前内存」写进日志（心跳）—— 真出事时日志里能看到最后状态；
//!   2. 内存超过上限就**主动安全停止**（走正常的拦截/结论路径），而不是等系统杀进程。
//!
//! 上限取法：配置 mem_limit_mb > 0 用它；=0 时自动按物理内存的 90%（留出系统余量）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// 心跳/检查间隔
pub const CHECK_SECS: u64 = 5;
/// 心跳日志间隔
pub const HEARTBEAT_SECS: u64 = 30;

#[derive(Default)]
pub struct MemGuard {
    /// 已触发内存上限
    pub exceeded: AtomicBool,
    /// 触发时的内存（MB），0=未触发
    pub peak_mb: AtomicU64,
}

impl MemGuard {
    pub fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Relaxed)
    }
    /// 触发原因（给 R 结论用）
    pub fn reason(&self, limit_mb: u64) -> String {
        format!("内存到达上限 {} MB（当前 {} MB），已安全停止", limit_mb, self.peak_mb.load(Ordering::Relaxed))
    }
}

/// 进程当前工作集（MB）
#[cfg(windows)]
pub fn working_set_mb() -> u64 {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn K32GetProcessMemoryInfo(process: *mut std::ffi::c_void, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
    }
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok == 0 {
        0
    } else {
        (c.working_set_size / (1024 * 1024)) as u64
    }
}

#[cfg(not(windows))]
pub fn working_set_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|r| r.parse::<u64>().ok()))
        .map(|pages| pages * 4096 / (1024 * 1024))
        .unwrap_or(0)
}

/// 物理内存总量（MB）
#[cfg(windows)]
pub fn total_phys_mb() -> u64 {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
    }
    let mut m = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        0
    } else {
        m.total_phys / (1024 * 1024)
    }
}

#[cfg(not(windows))]
pub fn total_phys_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal")).and_then(|l| l.split_whitespace().nth(1).and_then(|k| k.parse::<u64>().ok())))
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

/// 有效上限（MB）：配置优先，0=物理内存的 90%
pub fn effective_limit_mb(cfg_limit: i64) -> u64 {
    if cfg_limit > 0 {
        return cfg_limit as u64;
    }
    let total = total_phys_mb();
    if total == 0 {
        return 0;
    }
    total / 10 * 9
}

/// 看门狗句柄：worker 侧直接读 exceeded()
pub type Guard = Arc<MemGuard>;

pub fn new_guard() -> Guard {
    Arc::new(MemGuard::default())
}

/// 周期性检查：返回 true 表示"该继续跑"，false = 到上限了，请收尾
pub fn tick(guard: &Guard, limit_mb: u64, last_check: &mut Instant, label: &str, done: usize, total: usize, heartbeat: &mut Instant) -> bool {
    if last_check.elapsed().as_secs() < CHECK_SECS {
        return true;
    }
    *last_check = Instant::now();
    let now_mb = working_set_mb();
    if limit_mb > 0 && now_mb >= limit_mb {
        guard.exceeded.store(true, Ordering::Relaxed);
        guard.peak_mb.store(now_mb, Ordering::Relaxed);
        crate::core::app_dir::log_line(
            "findany-run.log",
            "err",
            &format!("{label}：内存 {now_mb} MB 已达上限 {limit_mb} MB（进度 {done}/{total}），主动安全停止"),
        );
        return false;
    }
    if heartbeat.elapsed().as_secs() >= HEARTBEAT_SECS {
        *heartbeat = Instant::now();
        crate::core::app_dir::log_line(
            "findany-run.log",
            "info",
            &format!("{label} 心跳：进度 {done}/{total}，内存 {now_mb} MB（上限 {limit_mb} MB）"),
        );
    }
    true
}
