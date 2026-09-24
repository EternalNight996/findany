//! 通用 CLI 回传：子进程调第三方 CLI（默认 intunehelper_cli.exe）逐台上传并判结果
//! （对齐 sonar/logfilter/uploader.py，判定规则移植 etest-core check_data 思路）。
//!
//! 退出码 ∈ success_codes 且 stdout JSON status ∈ status_ok -> ok（绿）
//! 退出码 ∈ conflict_codes                                 -> conflict（黄，转人工）
//! 退出码 ∈ retry_codes 仅重试；其余 / 判定失败             -> fail（红）

use serde_json::{Map, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const ST_OK: &str = "ok";
pub const ST_CONFLICT: &str = "conflict";
pub const ST_FAIL: &str = "fail";
pub const ST_DRY_RUN: &str = "dry_run";

pub const REQUIRED_FIELDS: [&str; 4] = ["serial_number", "product_key_id", "hardware_hash", "baseboard_product"];

/// payload 键 <- 提取字段名（etest(OA3) 提取器字段）。
pub fn default_field_map() -> Vec<(String, String)> {
    [
        ("serial_number", "sn"),
        ("product_key_id", "product_key_id"),
        ("hardware_hash", "hardware_hash"),
        ("baseboard_product", "baseboard_product"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

#[derive(Debug, Clone)]
pub struct UploadProfile {
    pub cli_path: String,
    /// 占位符 ~key~ 仿 etest rkey
    pub args: String,
    /// false 时 payload 经 ~payload~ 传参
    pub use_stdin: bool,
    pub timeout_sec: f64,
    /// 仅对 retry_codes 生效
    pub max_retries: i64,
    /// 退避基数：delay * 2^n
    pub retry_delay: f64,
    pub secret_key: String,
    pub success_codes: Vec<i64>,
    pub conflict_codes: Vec<i64>,
    pub retry_codes: Vec<i64>,
    pub status_ok: Vec<String>,
    pub field_map: Vec<(String, String)>,
}

impl Default for UploadProfile {
    fn default() -> Self {
        Self {
            cli_path: String::new(),
            args: "upload --stdin --secret-key ~secret_key~".into(),
            use_stdin: true,
            timeout_sec: 60.0,
            max_retries: 3,
            retry_delay: 1.0,
            secret_key: String::new(),
            success_codes: vec![0],
            conflict_codes: vec![12],
            retry_codes: vec![21],
            status_ok: vec!["accepted".into(), "duplicate_accepted".into()],
            field_map: default_field_map(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UploadResult {
    pub status: String,
    pub exit_code: Option<i64>,
    pub resp_status: String,
    pub request_id: String,
    pub attempts: i64,
    pub elapsed: f64,
    /// dry-run / 校验用；审计文件不落它（SOP 第 6 条）
    pub payload_json: String,
    pub error: String,
    pub stdout: String,
}

/// 按 field_map 组 payload；全部转字符串。
pub fn build_payload(fields: &Map<String, Value>, field_map: &[(String, String)]) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, src) in field_map {
        let v = match fields.get(src) {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(other) if other.is_null() => String::new(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        out.insert(key.clone(), Value::String(v));
    }
    out
}

pub fn missing_fields(payload: &Map<String, Value>) -> Vec<String> {
    REQUIRED_FIELDS
        .iter()
        .filter(|k| payload.get(**k).and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true))
        .map(|s| s.to_string())
        .collect()
}

/// 把 ~key~ 占位符渲染成实际参数。secret_key/payload 内置，其余查提取字段。
pub fn render_args(args: &str, profile: &UploadProfile, payload_json: &str, fields: &Map<String, Value>) -> Vec<String> {
    let mut mapping: Vec<(String, String)> = vec![
        ("secret_key".into(), profile.secret_key.clone()),
        ("payload".into(), payload_json.to_string()),
    ];
    for (k, v) in fields {
        mapping.push((k.clone(), match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }));
    }
    tokenize(args)
        .into_iter()
        .map(|tok| {
            let bytes = tok.as_bytes();
            if bytes.len() >= 3 && tok.starts_with('~') && tok.ends_with('~') {
                let key = &tok[1..tok.len() - 1];
                mapping
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or(tok)
            } else {
                tok
            }
        })
        .map(|t| t.trim_matches('"').to_string())
        .collect()
}

/// 简易 shlex 等价（posix=False 语义）：空格分词，引号内不拆（Windows 路径友好）。
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c.is_whitespace() {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// CLI 路径解析：显式路径 > 程序目录 > doc/devicehashupload（开发布局兜底）。
pub fn resolve_cli(cli_path: &str, app_dir: &str) -> String {
    if !cli_path.is_empty() && Path::new(cli_path).is_file() {
        return cli_path.to_string();
    }
    let mut cands: Vec<String> = Vec::new();
    if !app_dir.is_empty() {
        cands.push(Path::new(app_dir).join("intunehelper_cli.exe").to_string_lossy().to_string());
        cands.push(
            Path::new(app_dir)
                .join("doc")
                .join("devicehashupload")
                .join("intunehelper_cli.exe")
                .to_string_lossy()
                .to_string(),
        );
    }
    for c in cands {
        if Path::new(&c).is_file() {
            return c;
        }
    }
    // 开发布局再向上找几层：`cargo run` 时程序目录是 `<仓库>/target/debug`，
    // 仓库里的 doc/devicehashupload/intunehelper_cli.exe 就在上两层。
    if !app_dir.is_empty() {
        let mut cur = Path::new(app_dir);
        for _ in 0..3 {
            let Some(up) = cur.parent() else { break };
            cur = up;
            let cand = cur.join("doc").join("devicehashupload").join("intunehelper_cli.exe");
            if cand.is_file() {
                return cand.to_string_lossy().to_string();
            }
        }
    }
    cli_path.to_string()
}

/// 真实子进程调用。stdin 写完必须关闭（CLI 收到 EOF 才上传，SOP 第 4 节）。
///
/// `cancel` 可选：传入时每 20ms 检一次，cancel=true 立即 kill 子进程并返回 Err("cancelled")。
/// 取消粒度从"整台超时（默认 60s）"压到 ~20ms —— 解决"点停止后干等几十秒到几分钟才退出"。
pub fn default_runner(
    cmd: &[String],
    stdin_bytes: Option<Vec<u8>>,
    use_stdin: bool,
    timeout: f64,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(Option<i64>, String, String), String> {
    if cmd.is_empty() {
        return Err("CLI 路径为空".into());
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    c.stdin(if use_stdin { Stdio::piped() } else { Stdio::null() });
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = c.spawn().map_err(|e| format!("CLI 调用异常: {e}"))?;

    // stderr 单独线程读，避免双管道写满互锁
    let mut stderr_pipe = child.stderr.take();
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            use std::io::Read;
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    if use_stdin {
        if let Some(mut si) = child.stdin.take() {
            if let Some(data) = stdin_bytes {
                let _ = si.write_all(&data);
            }
            drop(si); // 关闭 stdin -> CLI 收到 EOF
        }
    }
    let mut stdout_buf = Vec::new();
    // stdout 也必须**在独立线程里读**：主线程 read_to_end 会一直等 EOF，而 CLI 若是启动器
    // （自己再拉起子进程、把管道也继承过去），EOF 可能永远不来 —— 主线程就永久阻塞在读取上，
    // 超时/取消全部失效，整批卡死（实测就是这么把 10248 个文件的任务卡住的）。
    let mut stdout_pipe = child.stdout.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stdout_pipe.as_mut() {
            use std::io::Read;
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.001));
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st.code().unwrap_or(-1) as i64),
            Ok(None) => {
                // 取消优先级最高：每 20ms 轮询一次，cancel=true 立即 kill
                if let Some(c) = cancel {
                    if c.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = err_handle.join();
                        return Err("cancelled".into());
                    }
                }
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = err_handle.join();
                    return Err("timeout".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("CLI 调用异常: {e}")),
        }
    };
    let stderr_buf = err_handle.join().unwrap_or_default();
    // 读线程最多再等 3 秒：管道若被 CLI 拉起的子进程攥着，EOF 永远不来 ——
    // 拿不到 stdout 就当没有（成功判定会退回退出码），绝不为了它把整批卡住。
    let wait_until = Instant::now() + Duration::from_secs(3);
    while !out_handle.is_finished() && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if out_handle.is_finished() {
        stdout_buf = out_handle.join().unwrap_or_default();
    }
    // 拿不到就留空：成功判定退回退出码，绝不为了 stdout 把整批卡住
    Ok((
        code,
        String::from_utf8_lossy(&stdout_buf).into_owned(),
        String::from_utf8_lossy(&stderr_buf).into_owned(),
    ))
}

/// 取 stdout 里最后一行 JSON（SOP：成功时单行结果 JSON）。
pub fn parse_stdout_json(stdout: &str) -> Option<Value> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.is_object() {
                return Some(v);
            }
        }
    }
    None
}

/// 上传一台。dry_run 只组包与校验，不打网不落地。
pub fn run_upload(
    profile: &UploadProfile,
    fields: &Map<String, Value>,
    dry_run: bool,
    on_log: &dyn Fn(&str, &str),
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> UploadResult {
    let payload = build_payload(fields, &profile.field_map);
    let miss = missing_fields(&payload);
    let payload_json = serde_json::to_string(&payload).unwrap_or_default();
    let mut res = UploadResult { status: ST_FAIL.into(), payload_json: payload_json.clone(), ..Default::default() };
    if !miss.is_empty() {
        res.error = format!("字段不全: {}", miss.join(","));
        return res;
    }
    if dry_run {
        res.status = ST_DRY_RUN.into();
        return res;
    }

    let cli = profile.cli_path.clone();
    if cli.is_empty() || !Path::new(&cli).is_file() {
        res.error = format!("CLI 不存在: {}", if cli.is_empty() { "(空)" } else { &cli });
        return res;
    }
    let mut cmd = vec![cli];
    cmd.extend(render_args(&profile.args, profile, &payload_json, fields));
    let stdin_bytes = payload_json.as_bytes().to_vec(); // UTF-8 无 BOM（SOP 4.1）

    let t0 = Instant::now();
    let max_try = (profile.max_retries + 1).max(1);
    for attempt in 1..=max_try {
        // 每轮重试前检一次 cancel（避免 max_retries=3 时连跑 3 次 dry-run 即使已取消）
        if let Some(c) = cancel {
            if c.load(std::sync::atomic::Ordering::SeqCst) {
                res.error = "cancelled".into();
                res.elapsed = round2(t0.elapsed().as_secs_f64());
                return res;
            }
        }
        res.attempts = attempt;
        let (rc, stdout, stderr) = match default_runner(&cmd, Some(stdin_bytes.clone()), profile.use_stdin, profile.timeout_sec, cancel) {
            Ok(v) => v,
            Err(e) => {
                res.error = e;
                res.elapsed = round2(t0.elapsed().as_secs_f64());
                return res;
            }
        };
        if stdout.is_empty() && stderr == "timeout" {
            res.error = "timeout".into();
            res.elapsed = round2(t0.elapsed().as_secs_f64());
            return res;
        }
        res.exit_code = rc;
        res.stdout = stdout.clone();
        let resp = parse_stdout_json(&stdout);
        res.resp_status = resp.as_ref().and_then(|v| v.get("status")).map(val_str).unwrap_or_default();
        res.request_id = resp.as_ref().and_then(|v| v.get("request_id")).map(val_str).unwrap_or_default();

        match rc {
            Some(code) if profile.success_codes.contains(&code) => {
                // 定版 ④A：双确认。退出码 0 且 stdout status 命中才 ok
                match &resp {
                    None => {
                        res.status = ST_FAIL.into();
                        res.error = "退出码 0 但 stdout 无结果 JSON".into();
                    }
                    Some(_) if profile.status_ok.contains(&res.resp_status) => {
                        res.status = ST_OK.into();
                    }
                    Some(_) => {
                        res.status = ST_FAIL.into();
                        res.error = format!(
                            "退出码 0 但 status={}",
                            if res.resp_status.is_empty() { "(空)" } else { &res.resp_status }
                        );
                    }
                }
                break;
            }
            Some(code) if profile.conflict_codes.contains(&code) => {
                res.status = ST_CONFLICT.into();
                res.error = format!("记录冲突(退出码 {code})，转人工：保留 request_id");
                break;
            }
            Some(code) if profile.retry_codes.contains(&code) && attempt < max_try => {
                let delay = profile.retry_delay * 2f64.powi((attempt - 1) as i32);
                on_log("warn", &format!("回传暂不可用(退出码 {code})，{delay:.0}s 后第 {} 次尝试", attempt + 1));
                if delay > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(delay));
                }
                continue;
            }
            other => {
                res.status = ST_FAIL.into();
                let code_s = other.map(|c| c.to_string()).unwrap_or_else(|| "None".into());
                let err = stderr.trim();
                res.error = if err.is_empty() {
                    format!("退出码 {code_s}")
                } else {
                    format!("退出码 {code_s}：{}", trim_chars(err, 200))
                };
                break;
            }
        }
    }
    res.elapsed = round2(t0.elapsed().as_secs_f64());
    res
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn trim_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
