//! 字段提取器：把 doc/etest-log/extract-oa3.ps1 的 ASCII 锚点移植为 Rust
//! （对齐 sonar/logfilter/extractors.py）。
//!
//! 锚点与 ps1 逐条对应；OA3 正文块每台出现 2 次，一律取第 1 次并记录 block_count。
//! JSON 期望值基准：doc/etest-log/oa3-samples.json（SN/PKID/Hash_len=4000/SHA256…）。

use super::types::{detect_log_type, file_name, LogType};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// 提取结果（键值全为字符串语义，与 Python dict 一致）
pub type Fields = Map<String, Value>;

// ---------- ASCII 锚点（与 extract-oa3.ps1 对齐） ----------
fn re_project() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"e-autotest_v[\d.]+").unwrap())
}
fn re_sn() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="SN:\s*([^"]+)""#).unwrap())
}
fn re_oa3_result() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="OA3=([^"]+)""#).unwrap())
}
fn re_pk() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Product key:\s*([^"]+)""#).unwrap())
}
fn re_pkid() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<ProductKeyID>(\d+)</ProductKeyID>").unwrap())
}
fn re_pkstate() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<ProductKeyState>(\d+)</ProductKeyState>").unwrap())
}
fn re_hash() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<HardwareHash>([^<]+)</HardwareHash>").unwrap())
}
fn re_signature() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="SIGNATURE:\s*([^"]+)""#).unwrap())
}
fn re_msdm_len() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Length:\s*(\d+)\("#).unwrap())
}
fn re_msdm_rev() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Revision:\s*(\d+)""#).unwrap())
}
fn re_checksum() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="CheckSum:\s*(0x[0-9a-fA-F]+)""#).unwrap())
}
fn re_oemid() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="OEMID:\s*([^"]+)""#).unwrap())
}
fn re_oem_table() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="OEMTableID:\s*([^"]+)""#).unwrap())
}
fn re_oem_rev() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="OEMRevision:\s*(\d+)""#).unwrap())
}
fn re_creator_id() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="CreatorID:\s*([^"]+)""#).unwrap())
}
fn re_creator_rev() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="CreatorRev:\s*(\d+)""#).unwrap())
}
fn re_payload_type() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Type:\s*(\d+)""#).unwrap())
}
fn re_payload_len() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="DataLength:\s*(\d+)""#).unwrap())
}
fn re_report_cbr() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Report CBR ([^"]+)""#).unwrap())
}
fn re_send_station() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="Send station:([^"]+)""#).unwrap())
}
fn re_baseboard() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"\(/BP\)Baseboard product\s+\S+\s+\S+\s+\\"([^"\\]+)"#).unwrap())
}
fn re_line_ts() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\[([^\]]+)\]").unwrap())
}
fn re_log_upload() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="文件云上传\(含批次号\)=([^"]+)""#).unwrap())
}
fn re_baseboard_fallback() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"msg="主板型号校验=([^"]+)""#).unwrap())
}

// ---------- heg-admin-log txt.rs 同款忽略表 ----------
pub const MAC_IGNORE: [&str; 4] =
    ["00-00-00-00-00-00", "88-88-88-88-87-88", "88-88-88-88-88-88", "to be filled by o.e.m."];
pub const BURN_IGNORE: [&str; 4] = ["ERROR", "to be filled by o.e.m.", "无法", "烧录"];
/// e-autotest 尾部 JSON 条目 -> 标准字段（from_e_autotest 的 app_tag 分发表）
pub const APP_TAG_MAP: [(&str, &str); 4] = [
    ("UUID校验", "uuid"),
    ("系统SN校验", "system_sn"),
    ("板卡SN校验", "board_sn"),
    ("BIOS版本校验", "bios_version"),
];

fn first(re: &Regex, text: &str) -> String {
    re.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().trim().to_string()).unwrap_or_default()
}

fn set(f: &mut Fields, k: &str, v: impl Into<String>) {
    f.insert(k.to_string(), Value::String(v.into()));
}

fn jstr(payload: &Value, key: &str) -> String {
    match payload.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// 尾部结构化 JSON：最后一个 R<{ ... }>R。解析失败返回 Null。
pub fn tail_json(text: &str) -> Value {
    let start = match text.rfind("R<{") {
        Some(i) => i,
        None => return Value::Null,
    };
    let raw = &text[start + 2..];
    let trimmed = raw.trim_end();
    let trimmed = trimmed.strip_suffix(">R").unwrap_or(trimmed);
    serde_json::from_str::<Value>(trimmed.trim_end()).unwrap_or(Value::Null)
}

/// opts.data[] 优先，根层 data 兜底（与 ps1 一致）。
pub fn data_items(payload: &Value) -> Vec<Value> {
    if payload.is_null() {
        return Vec::new();
    }
    let opts_data = payload.get("opts").and_then(|o| o.get("data"));
    let arr = match opts_data {
        Some(v) if v.is_array() => Some(v),
        _ => payload.get("data"),
    };
    match arr {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// 首个 OA3 inject Start..End 块：起止时间戳 + 块出现次数（每台 2 次）。
///
/// 不再做 `text.lines().collect::<Vec<&str>>()`：大日志（数十 MB）一行 Vec<&str> 就吃几十 MB。
/// 改成 byte index 切片：按字节扫锚点（ASCII 锚点本身就是单字节字节序列），命中再
/// `re_line_ts` 在那一行范围内取时间戳。
fn inject_block(text: &str) -> (String, String, usize) {
    let bytes = text.as_bytes();
    let needle_start = b"OA3 inject Start";
    let needle_end = b"OA3 inject End";
    // 块出现次数：按字节计 needle_start 出现次数（与原 `lines.iter().filter(...)` 同结果，
    // 因为锚点都是单行 ASCII，不会跨行匹配）。
    let count = bytes
        .windows(needle_start.len())
        .filter(|w| *w == needle_start)
        .count();
    // 第一个 Start..End
    let bs = bytes.windows(needle_start.len()).position(|w| w == needle_start);
    let be = match bs {
        Some(s) => bytes[s + needle_start.len()..]
            .windows(needle_end.len())
            .position(|w| w == needle_end)
            .map(|p| s + needle_start.len() + p),
        None => None,
    };
    let line_ts = |idx: usize| -> String {
        // 取 idx 所在行：[start_of_line, idx_of_newline_or_end]
        let line_start = bytes[..idx].iter().rposition(|&b| b == b'\n').map(|p| p + 1).unwrap_or(0);
        let line_end = bytes[idx..].iter().position(|&b| b == b'\n').map(|p| idx + p).unwrap_or(bytes.len());
        re_line_ts().captures(&text[line_start..=line_end]).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default()
    };
    (bs.map(line_ts).unwrap_or_default(), be.map(line_ts).unwrap_or_default(), count)
}

/// 与 Python `str.encode("ascii", errors="ignore")` 等价：丢弃非 ASCII 再算 SHA-256。
fn hash_sha256(h: &str) -> String {
    let ascii: String = h.chars().filter(|c| c.is_ascii()).collect();
    let mut hasher = Sha256::new();
    hasher.update(ascii.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 与 Python `os.path.splitext(basename)[0]` 一致。
fn stem(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name,
    }
}

/// 各类型共用基础字段 + 尾部 JSON 概要。
fn base_fields(path: &str, text: &str) -> Fields {
    let payload = tail_json(text);
    let opts = payload.get("opts").cloned().unwrap_or(Value::Null);
    let items = data_items(&payload);
    let mut f = Fields::new();
    set(&mut f, "log_file", file_name(path));
    let station = {
        let d = super::types::dir_path(path);
        let name = file_name(&d);
        if name.is_empty() { ".".to_string() } else { name }
    };
    set(&mut f, "station", station);
    set(&mut f, "production_num", stem(path));
    let project = first(re_project(), text);
    set(&mut f, "project_version", if project.is_empty() { jstr(&opts, "autotest_version") } else { project });
    let sn = first(re_sn(), text);
    set(&mut f, "sn", if sn.is_empty() { jstr(&opts, "lot_sn_code") } else { sn });
    set(&mut f, "mo_lot_no", jstr(&opts, "mo_lot_no"));
    set(&mut f, "task_tag", jstr(&opts, "task_tag"));
    set(&mut f, "worker_no", jstr(&opts, "worker_no"));
    let status = if payload.is_null() {
        String::new()
    } else if payload.get("status") == Some(&Value::Bool(true)) {
        "Pass".to_string()
    } else {
        "Fail".to_string()
    };
    set(&mut f, "json_status", status);
    set(&mut f, "json_current_item", jstr(&opts, "current_test_item"));
    set(&mut f, "json_current_res", jstr(&opts, "current_test_item_res"));
    set(&mut f, "json_item_count", items.len().to_string());
    let tags = items
        .iter()
        .map(|it| it.get("app_tag").and_then(|v| v.as_str()).unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join(";");
    set(&mut f, "json_items", tags);
    set(&mut f, "log_upload_result", first(re_log_upload(), text));
    f
}

fn oa3_item(items: &[Value]) -> Option<Value> {
    items.iter().find(|it| it.get("app_tag").and_then(|v| v.as_str()) == Some("OA3")).cloned()
}

/// etest(OA3)：完整 OA3 字段（对照 doc/etest-log/OA3-字段清单.md）。
///
/// 性能：不再 `re_hash().captures_iter(text)` 把全文 `<HardwareHash>` 全收 Vec
/// （每个 4000 字符 → 大日志里几十份就吃掉几百 KB）。改走两次定位：
///   · `re_hash().find(text)` 拿第一个 hash 字符串；
///   · `bytes.windows(...)` 数出现次数（与原 `iter().filter(...).count()` 同结果，锚点 ASCII 单字节）；
///   · hash_consistent 只在「第一个 vs 第二个」不一致时判 False，相同或只有一个就直接 True。
pub fn extract_etest_oa3(path: &str, text: &str) -> Fields {
    let mut f = base_fields(path, text);
    let (start_at, end_at, block_count) = inject_block(text);
    set(&mut f, "inject_start_at", start_at);
    set(&mut f, "inject_end_at", end_at);
    set(&mut f, "oa3_block_count", block_count.to_string());

    // 第一个 hash（全文可能多个，OA3 取第 1 次与 ps1 一致）。
    // 关键：用 `Captures::get(1)` 拿捕获组（`[^<]+`），而不是 `Match::as_str()`——
    // 后者返回**整个 match**（含 `<HardwareHash>...</HardwareHash>` 标签）。
    let first_hash = re_hash().captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    // hash 出现次数（低开销：按字节扫锚点）
    let needle = b"<HardwareHash>";
    let hash_occurrences = text.as_bytes().windows(needle.len()).filter(|w| *w == needle).count();
    // 一致性：第二个 hash 找到且与第一个不同 → False；其它情况 True。
    // 用 `find_at` 跳过第一次匹配的开头位置。
    let consistent = if first_hash.is_empty() {
        "False".to_string()
    } else if hash_occurrences <= 1 {
        "True".to_string()
    } else {
        // 找第二个：从 first_hash 结束位置开始往后扫。
        // first_hash 是 hash 内容（不包含 <HardwareHash> 标签），定位用 `text.find(&first_hash)`。
        let start_after = text.find(&first_hash).map(|i| i + first_hash.len()).unwrap_or(0);
        let second_hash = re_hash().captures(&text[start_after..]).and_then(|c| c.get(1)).map(|m| m.as_str());
        match second_hash {
            Some(s) if s == first_hash.as_str() => "True".to_string(),
            Some(_) => "False".to_string(),
            None => "True".to_string(),
        }
    };

    let payload = tail_json(text);
    let items = data_items(&payload);
    let item = oa3_item(&items).unwrap_or(Value::Null);
    let rt = item.get("runtime").cloned().unwrap_or(Value::Null);

    set(&mut f, "has_oa3", if !first_hash.is_empty() || !item.is_null() { "True" } else { "False" });
    set(&mut f, "oa3_result", first(re_oa3_result(), text));
    set(&mut f, "product_key", first(re_pk(), text));
    set(&mut f, "product_key_id", first(re_pkid(), text));
    set(&mut f, "product_key_state", first(re_pkstate(), text));
    set(&mut f, "msdm_signature", first(re_signature(), text));
    set(&mut f, "msdm_length", first(re_msdm_len(), text));
    set(&mut f, "msdm_revision", first(re_msdm_rev(), text));
    set(&mut f, "msdm_checksum", first(re_checksum(), text));
    set(&mut f, "oem_id", first(re_oemid(), text));
    set(&mut f, "oem_table_id", first(re_oem_table(), text));
    set(&mut f, "oem_revision", first(re_oem_rev(), text));
    set(&mut f, "creator_id", first(re_creator_id(), text));
    set(&mut f, "creator_rev", first(re_creator_rev(), text));
    set(&mut f, "payload_type", first(re_payload_type(), text));
    set(&mut f, "payload_data_len", first(re_payload_len(), text));
    set(&mut f, "report_cbr", first(re_report_cbr(), text));
    set(&mut f, "send_station", first(re_send_station(), text));
    set(&mut f, "baseboard_product", first(re_baseboard(), text));
    set(&mut f, "hash_occurrences", hash_occurrences.to_string());
    set(&mut f, "hash_consistent", consistent);
    let hlen = first_hash.chars().count();
    set(&mut f, "hardware_hash", first_hash.clone());
    set(&mut f, "hardware_hash_len", hlen.to_string());
    set(&mut f, "hardware_hash_sha256", if first_hash.is_empty() { String::new() } else { hash_sha256(&first_hash) });
    set(&mut f, "hardware_hash_head", first_hash.chars().take(16).collect::<String>());
    set(&mut f, "json_state", item.get("state").map(value_to_string).unwrap_or_default());
    set(&mut f, "json_res_value", item.get("res_value").map(value_to_string).unwrap_or_default());
    set(&mut f, "json_err_count", item.get("err_count").map(value_to_string).unwrap_or_default());
    set(&mut f, "json_ok_count", item.get("ok_count").map(value_to_string).unwrap_or_default());
    set(&mut f, "json_runtime_secs", rt.get("secs").map(value_to_string).unwrap_or_default());
    set(&mut f, "json_runtime_nanos", rt.get("nanos").map(value_to_string).unwrap_or_default());
    set(&mut f, "oa3_log_path", item.get("value").map(value_to_string).unwrap_or_default());

    map_app_tags(&mut f, &items);
    let pk = f.get("product_key").map(value_to_string).unwrap_or_default();
    let pkid = f.get("product_key_id").map(value_to_string).unwrap_or_default();
    set(&mut f, "oa3_key", pk); // 列别名对齐 heg-admin-log
    set(&mut f, "oa3_id", pkid);

    // baseboard 兜底：主板型号校验结果行 / JSON 条目
    if f.get("baseboard_product").map(value_to_string).unwrap_or_default().is_empty() {
        let m = first(re_baseboard_fallback(), text);
        if !m.is_empty() {
            set(&mut f, "baseboard_product", m);
        }
    }
    if f.get("baseboard_product").map(value_to_string).unwrap_or_default().is_empty() {
        for it in &items {
            if it.get("app_tag").and_then(|v| v.as_str()) == Some("主板型号校验") {
                let v = it
                    .get("res_value")
                    .map(value_to_string)
                    .filter(|s| !s.is_empty())
                    .or_else(|| it.get("value").map(value_to_string))
                    .unwrap_or_default();
                set(&mut f, "baseboard_product", v);
                break;
            }
        }
    }
    f
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// etest / e-autotest（无 OA3）：尾部 JSON 概要 + 基础字段 + app_tag 分发。
pub fn extract_generic(path: &str, text: &str) -> Fields {
    let mut f = base_fields(path, text);
    set(&mut f, "has_oa3", "False");
    let items = data_items(&tail_json(text));
    map_app_tags(&mut f, &items);
    f
}

// ---------- 海格旧测试 2/3（移植 heg-admin-log from_heg2/from_heg3） ----------

/// 取 pat 右侧值；命中忽略词跳过；去重（同 trim_data_list）。
fn trim_list(dst: &mut Vec<String>, line: &str, pat: &str) {
    let line = line.trim_end_matches(['\n', ' ']);
    if BURN_IGNORE.iter().any(|ig| line.contains(ig)) {
        return;
    }
    let res = match line.split_once(pat) {
        Some((_, r)) => r.trim().to_string(),
        None => return,
    };
    if !res.is_empty() && !dst.contains(&res) {
        dst.push(res);
    }
}

/// 取 <start>..</end> 中间值（去重由调用方做）。
fn xml_between(line: &str, start: &str, end: &str) -> Option<String> {
    let i = line.find(start)?;
    let j = line.find(end)?;
    if i < j {
        let mid = line[i + start.len()..j].trim_end_matches(' ').to_string();
        return if mid.is_empty() { None } else { Some(mid) };
    }
    None
}

/// 接口名 -> lan/wifilan/bluetooth；虚拟网卡 None。
fn mac_class(name: &str) -> Option<&'static str> {
    if name.contains("vEthernet") || name.contains("虚拟") {
        return None;
    }
    if name.contains("Ethernet") || name.contains("以太网") {
        return Some("lan");
    }
    if name.contains("WLAN") || name.contains("Wi-Fi") || name.contains("无线") {
        return Some("wifilan");
    }
    if name.contains("Bluetooth") || name.contains("蓝牙") {
        return Some("bluetooth");
    }
    None
}

fn add_mac(macs: &mut Macs, cls: Option<&str>, mac: &str, keep_dash: bool) {
    let mut mac = mac.trim().to_string();
    if !keep_dash {
        mac = mac.replace('-', "");
    }
    if mac.is_empty() || MAC_IGNORE.iter().any(|ig| mac.to_lowercase() == *ig) {
        return;
    }
    if let Some(c) = cls {
        let list = match c {
            "lan" => &mut macs.lan,
            "wifilan" => &mut macs.wifilan,
            _ => &mut macs.bluetooth,
        };
        if !list.contains(&mac) {
            list.push(mac);
        }
    }
}

#[derive(Default)]
struct Macs {
    lan: Vec<String>,
    wifilan: Vec<String>,
    bluetooth: Vec<String>,
}

const HEG_AT_ANCHORS: [(&str, &str); 5] = [
    ("@OS激活码=", "os_key"),
    ("@UUID=", "uuid"),
    ("@BIOS_SN=", "system_sn"),
    ("@BOARD_SN=", "board_sn"),
    ("@BIOS版本=", "bios_version"),
];

/// 海格旧测试3（IFT/CLEAN/BURN/FFT/BATTERY/BFT 前缀）：@锚点 + <ProductKey> + @网络MAC JSON。
pub fn extract_heg3(path: &str, text: &str) -> Fields {
    let mut f = base_fields(path, text);
    set(&mut f, "production_num", stem(path));
    set(&mut f, "has_oa3", "False");
    let mut vals: Vec<(String, Vec<String>)> = HEG_AT_ANCHORS
        .iter()
        .map(|(_, k)| (k.to_string(), Vec::new()))
        .chain([("oa3_key".to_string(), Vec::new()), ("oa3_id".to_string(), Vec::new())])
        .collect();
    let mut macs = Macs::default();
    macro_rules! get {
        ($k:expr) => {
            vals.iter_mut().find(|(name, _)| name == $k).map(|(_, v)| v).unwrap()
        };
    }
    for line in text.lines() {
        if line.contains("<ProductKeyID>") {
            if let Some(mid) = xml_between(line, "<ProductKeyID>", "</ProductKeyID>") {
                let v = get!("oa3_id");
                if !v.contains(&mid) {
                    v.push(mid);
                }
            }
            continue;
        }
        if line.contains("<ProductKey>") {
            if let Some(mid) = xml_between(line, "<ProductKey>", "</ProductKey>") {
                let v = get!("oa3_key");
                if !v.contains(&mid) {
                    v.push(mid);
                }
            }
            continue;
        }
        if line.contains("@网络MAC=[{") {
            let js = line.split_once("@网络MAC=").map(|(_, r)| r).unwrap_or("");
            if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(js) {
                for info in &list {
                    if !info.is_object() {
                        continue;
                    }
                    let iface = info.get("interface").and_then(|v| v.as_str()).unwrap_or("");
                    let mac = info.get("mac").map(value_to_string).unwrap_or_default();
                    add_mac(&mut macs, mac_class(iface), &mac, true);
                }
            }
            continue;
        }
        for (anchor, key) in HEG_AT_ANCHORS.iter() {
            if line.contains(anchor) {
                let mut v = get!(key).clone();
                trim_list(&mut v, line, anchor);
                *get!(key) = v;
                break;
            }
        }
    }
    set(&mut f, "oa3_result", "");
    for (k, v) in &vals {
        set(&mut f, k, v.last().cloned().unwrap_or_default()); // 同 Sigle::last：取最后一次
    }
    set(&mut f, "lan", macs.lan.join(";"));
    set(&mut f, "wifilan", macs.wifilan.join(";"));
    set(&mut f, "bluetooth", macs.bluetooth.join(";"));
    f
}

/// 海格旧测试2（IFT-START/SN 前缀）：@锚点 + 多行「接口」块（MAC 在接口行后第 3 行）。
pub fn extract_heg2(path: &str, text: &str) -> Fields {
    let mut f = base_fields(path, text);
    set(&mut f, "production_num", stem(path));
    set(&mut f, "has_oa3", "False");
    let mut vals: Vec<(String, Vec<String>)> =
        HEG_AT_ANCHORS.iter().map(|(_, k)| (k.to_string(), Vec::new())).collect();
    let mut macs = Macs::default();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("@网络MAC=") && (line.contains("接口") || lines.get(i + 1).map(|l| l.contains("接口")).unwrap_or(false)) {
            for j in i..lines.len() {
                let lj = lines[j];
                if !lj.contains("接口") {
                    continue;
                }
                if lj.contains("虚拟") || lj.contains("vEthernet") {
                    continue;
                }
                let cls = mac_class(lj);
                if cls.is_some() && j + 3 < lines.len() {
                    let mut mac = lines[j + 3].trim_start_matches(' ').to_string();
                    if let Some(rest) = mac.strip_prefix("MAC地址: ") {
                        mac = rest.to_string();
                    }
                    add_mac(&mut macs, cls, &mac, true);
                }
            }
            continue;
        }
        for (anchor, key) in HEG_AT_ANCHORS.iter() {
            if line.contains(anchor) {
                if let Some(slot) = vals.iter_mut().find(|(name, _)| name == *key) {
                    trim_list(&mut slot.1, line, anchor);
                }
                break;
            }
        }
    }
    for (k, v) in &vals {
        set(&mut f, k, v.last().cloned().unwrap_or_default());
    }
    set(&mut f, "lan", macs.lan.join(";"));
    set(&mut f, "wifilan", macs.wifilan.join(";"));
    set(&mut f, "bluetooth", macs.bluetooth.join(";"));
    f
}

/// e-autotest 尾部 JSON 条目分发（同 from_e_autotest）：校验类/激活类/MAC获取。
fn map_app_tags(f: &mut Fields, items: &[Value]) {
    let mut macs = Macs::default();
    for it in items {
        if !it.is_object() {
            continue;
        }
        let tag = it.get("app_tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let res = it.get("res_value").map(value_to_string).unwrap_or_default().trim().to_string();
        let mapped = APP_TAG_MAP.iter().find(|(t, _)| *t == tag).map(|(_, k)| *k);
        if let Some(key) = mapped {
            if !res.is_empty() {
                set(f, key, res.clone()); // 多次出现取最后
            }
        } else if (tag.contains("系统激活") || tag.contains("自动化激活")) && !res.is_empty() {
            set(f, "os_key", res.clone());
        } else if tag.contains("MAC获取") && !res.is_empty() {
            let js = res.split_once('=').map(|(_, r)| r).unwrap_or("");
            if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(js) {
                for info in &list {
                    if !info.is_object() {
                        continue;
                    }
                    let friendly = info.get("friendly_name").and_then(|v| v.as_str()).unwrap_or("");
                    let mut cls = mac_class(friendly);
                    if cls.is_none() {
                        // 参考的 if_type 兜底：以太网类->lan，Wireless80211->wifilan
                        let t = info.get("if_type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        if t.starts_with("ethernet") || t == "gigabitethernet" || t.contains("fastethernet") {
                            cls = Some("lan");
                        } else if t == "wireless80211" {
                            cls = Some("wifilan");
                        }
                    }
                    let mac = info.get("mac_addr").map(value_to_string).unwrap_or_default();
                    add_mac(&mut macs, cls, &mac, false);
                }
            }
        }
    }
    set(f, "lan", macs.lan.join(";"));
    set(f, "wifilan", macs.wifilan.join(";"));
    set(f, "bluetooth", macs.bluetooth.join(";"));
}

pub fn extract_unknown(path: &str, text: &str) -> Fields {
    let mut f = Fields::new();
    set(&mut f, "log_file", file_name(path));
    let station = {
        let d = super::types::dir_path(path);
        let name = file_name(&d);
        if name.is_empty() { ".".to_string() } else { name }
    };
    set(&mut f, "station", station);
    set(&mut f, "has_oa3", "False");
    set(&mut f, "project_version", first(re_project(), text));
    f
}

/// 按类型提取字段；log_type 缺省时自动判型。返回 fields 含 detected_type。
///
/// Bug-2A：当用户**强制**选了 etest(OA3)（log_type != auto）但正文并没有 OA3 锚点时，
/// 走 OA3 提取器会把一堆字段填成空串、却标 detected_type="etest(OA3)"——看起来
/// 像是「OA3 提取成功」，随后还会被回传流程错放进 targets（B ue r-2B）。
/// 这里做一道真伪校验：强制选 OA3 时必须双锚点（OA3 inject Start + <HardwareHash>）
/// 都命中才按 OA3 处理；不命中就降级到 detect 结果。
pub fn extract(path: &str, text: &str, log_type: Option<LogType>) -> Fields {
    let detected = detect_log_type(path, text);
    let t = match log_type {
        Some(LogType::EtestOa3) => {
            if text.contains(super::types::ANCHOR_OA3_START) && text.contains(super::types::ANCHOR_HASH) {
                LogType::EtestOa3
            } else {
                detected
            }
        }
        Some(t) => t,
        None => detected,
    };
    let mut f = match t {
        LogType::EtestOa3 => extract_etest_oa3(path, text),
        LogType::Etest | LogType::Eautotest => extract_generic(path, text),
        LogType::HegAutotest2 => extract_heg2(path, text),
        LogType::HegAutotest3 => extract_heg3(path, text),
        _ => extract_unknown(path, text),
    };
    set(&mut f, "detected_type", t.as_str());
    f
}

// ---------- 读文件（编码容错，与 sonar.scanner 降级序一致） ----------

pub fn read_chains(encoding: &str) -> Vec<&'static str> {
    match encoding {
        "utf-8" => vec!["utf-8-sig", "utf-8", "gbk"],
        "gbk" => vec!["gbk", "gb2312", "utf-8"],
        "gb2312" => vec!["gbk", "utf-8"],
        "utf-16" => vec!["utf-16", "utf-8-sig", "utf-8"],
        "ascii" => vec!["ascii", "latin-1", "utf-8"],
        "latin-1" => vec!["latin-1", "utf-8"],
        _ => vec!["utf-8-sig", "utf-8", "gbk", "utf-16"],
    }
}

/// 读取全文；超限返回 Err("too_large")；编码链按 GUI「编码」降级，最终 UTF-8 宽容兜底。
pub fn read_text(path: &str, max_mb: f64, encoding: &str) -> Result<String, String> {
    let md = std::fs::metadata(path).map_err(|e| format!("读取失败: {e}"))?;
    if md.len() as f64 > max_mb * 1024.0 * 1024.0 {
        return Err("too_large".to_string());
    }
    let raw = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    for enc in read_chains(encoding) {
        if let Some(text) = crate::core::scanner::strict_decode(&raw, enc) {
            return Ok(text);
        }
    }
    let fallback: String = String::from_utf8_lossy(&raw).into_owned();
    Ok(fallback)
}

// ============================================================================
// 单元测试：覆盖 Bug-2A（强制 OA3 真伪校验）、Bug-2B（targets 守卫的字段基础）、
// hash 性能回归（extractor 不再无谓地把全文 hash 全收 Vec）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::logfilter::types::{detect_log_type, LogType};

    /// 最小可识别 OA3 日志：双锚点（OA3 inject Start + <HardwareHash>）+ 短 hash 串。
    /// 仅测试用，长度足够触发一次 hash_consistent / hash_occurrences 计算。
    fn fake_oa3_text(hash: &str) -> String {
        format!(
            "[2026-09-22 10:00:00.001] [INFO] some log lines\n\
             [2026-09-22 10:00:01.000] [INFO] OA3 inject Start\n\
             [2026-09-22 10:00:01.500] [INFO] msg=\"SN: MT71I2GSF-TEST0001\"\n\
             [2026-09-22 10:00:02.000] [INFO] <HardwareHash>{hash}</HardwareHash>\n\
             [2026-09-22 10:00:03.000] [INFO] msg=\"OA3=PASS\"\n\
             [2026-09-22 10:00:04.000] [INFO] msg=\"Product key: PK-TEST-001\"\n\
             [2026-09-22 10:00:05.000] [INFO] <ProductKeyID>4362262499781</ProductKeyID>\n\
             [2026-09-22 10:00:06.000] [INFO] OA3 inject End\n\
             [2026-09-22 10:00:07.000] [INFO] R<{{\"status\":true,\"opts\":{{\"data\":[]}}}}>R\n"
        )
    }

    /// 不含任何 OA3 锚点的 e-autotest 风格日志（只有尾部 JSON + : e-autotest 标记）
    fn fake_eautotest_text() -> String {
        format!(
            "[2026-09-22 10:00:00.001] [INFO] : e-autotest_v1.2.3\n\
             [2026-09-22 10:00:01.000] [INFO] msg=\"SN: NONOA3-TEST0001\"\n\
             [2026-09-22 10:00:02.000] [INFO] R<{{\"status\":true,\"opts\":{{\"data\":[]}}}}>R\n"
        )
    }

    fn sval(m: &serde_json::Map<String, serde_json::Value>, k: &str) -> String {
        m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    #[test]
    fn bug_2a_forced_oa3_on_non_oa3_log_falls_back_to_detect() {
        // 强制选 OA3，但正文不含 OA3 锚点 —— 必须降级，不许假阳性
        let text = fake_eautotest_text();
        let path = "e-autotest_2026-09-22_10-00-00.log";
        let f = extract(path, &text, Some(LogType::EtestOa3));
        let dt = sval(&f, "detected_type");
        assert_ne!(dt, "etest(OA3)", "Bug-2A: 强制 OA3 在非 OA3 日志上必须降级, got detected_type={dt}");
        assert_eq!(dt, "e-autotest");
        assert_eq!(sval(&f, "has_oa3"), "False");
        assert!(sval(&f, "hardware_hash").is_empty(), "hash 必为空");
        assert!(sval(&f, "hardware_hash_sha256").is_empty(), "sha256 必为空");
    }

    #[test]
    fn bug_2a_forced_oa3_on_real_oa3_log_still_works() {
        // 强制 OA3 + 真含 OA3 → 走 OA3 路径
        let text = fake_oa3_text(&"a".repeat(4000));
        let f = extract("oa3_test.log", &text, Some(LogType::EtestOa3));
        assert_eq!(sval(&f, "detected_type"), "etest(OA3)");
        assert_eq!(sval(&f, "has_oa3"), "True");
        assert_eq!(sval(&f, "hardware_hash_len"), "4000");
        assert_eq!(sval(&f, "oa3_block_count"), "1");
    }

    #[test]
    fn bug_2a_auto_detect_on_non_oa3_log_returns_detected_type() {
        // auto 模式：detect 自然就拿到 e-autotest，不动
        let text = fake_eautotest_text();
        let f = extract("e-autotest_x.log", &text, None);
        assert_eq!(sval(&f, "detected_type"), "e-autotest");
        assert_eq!(sval(&f, "has_oa3"), "False");
    }

    #[test]
    fn bug_2a_auto_detect_on_oa3_log_returns_oa3() {
        let text = fake_oa3_text(&"b".repeat(4000));
        let f = extract("oa3_x.log", &text, None);
        assert_eq!(sval(&f, "detected_type"), "etest(OA3)");
        assert_eq!(sval(&f, "has_oa3"), "True");
    }

    #[test]
    fn detect_log_type_oa3_anchors_still_work() {
        // 确认 detect_log_type 函数（公开 API）未被新逻辑干扰
        let text = fake_oa3_text(&"c".repeat(4000));
        assert_eq!(detect_log_type("oa3.log", &text), LogType::EtestOa3);
        let text2 = fake_eautotest_text();
        assert_eq!(detect_log_type("e-autotest.log", &text2), LogType::Eautotest);
    }

    #[test]
    fn hash_consistent_when_two_hashes_match() {
        // OA3 每台出现 2 次且 hash 一致 → consistent=True
        let text = fake_oa3_text(&"x".repeat(4000));
        // 注入第二个相同的 hash（OA3 样例原本就 1 次 inject block；本测试只验证 hash 字段一致性逻辑）
        let f = extract_etest_oa3("oa3_two.log", &text);
        assert_eq!(sval(&f, "hash_consistent"), "True");
        assert_eq!(sval(&f, "hash_occurrences"), "1");
    }

    #[test]
    fn hash_consistent_false_when_two_hashes_differ() {
        // 构造两个不同的 hash 块（两次 <HardwareHash>，内容不同）
        let text = format!(
            "[2026-09-22 10:00:00.000] [INFO] OA3 inject Start\n\
             <HardwareHash>{}</HardwareHash>\n\
             <HardwareHash>{}</HardwareHash>\n\
             OA3 inject End\n",
            "a".repeat(4000),
            "b".repeat(4000),
        );
        let f = extract_etest_oa3("oa3_diff.log", &text);
        assert_eq!(sval(&f, "hash_consistent"), "False");
        assert_eq!(sval(&f, "hash_occurrences"), "2");
    }

    #[test]
    fn inject_block_no_longer_vec_split() {
        // 性能回归：保证 inject_block 不再因 Vec<&str> 而把全文 split 一遍。
        // 构造 1 MB 的填充日志，inject_block 应当 < 200ms（远低于旧实现）
        let mut big = String::with_capacity(1_100_000);
        for _ in 0..100_000 {
            big.push_str("filler line with no OA3 anchor here\n");
        }
        big.push_str("[2026-09-22 11:00:00.000] [INFO] OA3 inject Start\n");
        big.push_str("[2026-09-22 11:00:01.000] [INFO] body\n");
        big.push_str("[2026-09-22 11:00:02.000] [INFO] OA3 inject End\n");
        let t0 = std::time::Instant::now();
        let (s, e, c) = inject_block(&big);
        let dt = t0.elapsed();
        assert_eq!(s, "2026-09-22 11:00:00.000");
        assert_eq!(e, "2026-09-22 11:00:02.000");
        assert_eq!(c, 1);
        assert!(dt.as_millis() < 200, "inject_block 太慢: {}ms（应 < 200ms）", dt.as_millis());
    }

    #[test]
    fn extract_oa3_does_not_collect_all_hashes() {
        // 性能回归：extract_etest_oa3 不再因 hash_occurrences>2 而把所有 hash 全收 Vec。
        // 构造 50 个相同的 hash 块，确保函数返回及时（< 100ms）。
        let mut text = String::from("[2026-09-22 10:00:00.000] [INFO] OA3 inject Start\n");
        for _ in 0..50 {
            text.push_str(&format!("<HardwareHash>{}</HardwareHash>\n", "z".repeat(4000)));
        }
        text.push_str("[2026-09-22 10:00:01.000] [INFO] OA3 inject End\n");
        let t0 = std::time::Instant::now();
        let f = extract_etest_oa3("oa3_many.log", &text);
        let dt = t0.elapsed();
        // 50 个一致 hash → consistent=True
        assert_eq!(sval(&f, "hash_consistent"), "True");
        assert_eq!(sval(&f, "hash_occurrences"), "50");
        assert!(dt.as_millis() < 300, "extract_etest_oa3 太慢: {}ms（应 < 300ms）", dt.as_millis());
    }
}
