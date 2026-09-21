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
fn inject_block(lines: &[&str]) -> (String, String, usize) {
    let mut bs: i64 = -1;
    let mut be: i64 = -1;
    for (i, line) in lines.iter().enumerate() {
        if bs < 0 && line.contains("OA3 inject Start") {
            bs = i as i64;
        } else if bs >= 0 && line.contains("OA3 inject End") {
            be = i as i64;
            break;
        }
    }
    let count = lines.iter().filter(|l| l.contains("OA3 inject Start")).count();
    let ts = |idx: i64| -> String {
        if idx < 0 {
            return String::new();
        }
        re_line_ts().captures(lines[idx as usize]).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default()
    };
    (ts(bs), ts(be), count)
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
pub fn extract_etest_oa3(path: &str, text: &str) -> Fields {
    let mut f = base_fields(path, text);
    let lines: Vec<&str> = text.lines().collect();
    let (start_at, end_at, block_count) = inject_block(&lines);
    set(&mut f, "inject_start_at", start_at);
    set(&mut f, "inject_end_at", end_at);
    set(&mut f, "oa3_block_count", block_count.to_string());

    let hashes: Vec<String> = re_hash().captures_iter(text).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect();
    let h = hashes.first().cloned().unwrap_or_default();
    let payload = tail_json(text);
    let items = data_items(&payload);
    let item = oa3_item(&items).unwrap_or(Value::Null);
    let rt = item.get("runtime").cloned().unwrap_or(Value::Null);

    set(&mut f, "has_oa3", if !hashes.is_empty() || !item.is_null() { "True" } else { "False" });
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
    set(&mut f, "hash_occurrences", hashes.len().to_string());
    let consistent = if hashes.is_empty() {
        "False".to_string()
    } else {
        let uniq: std::collections::HashSet<&String> = hashes.iter().collect();
        if uniq.len() <= 1 { "True".to_string() } else { "False".to_string() }
    };
    set(&mut f, "hash_consistent", consistent);
    let hlen = h.chars().count();
    set(&mut f, "hardware_hash", h.clone());
    set(&mut f, "hardware_hash_len", hlen.to_string());
    set(&mut f, "hardware_hash_sha256", if h.is_empty() { String::new() } else { hash_sha256(&h) });
    set(&mut f, "hardware_hash_head", h.chars().take(16).collect::<String>());
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
pub fn extract(path: &str, text: &str, log_type: Option<LogType>) -> Fields {
    let t = log_type.unwrap_or_else(|| detect_log_type(path, text));
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
