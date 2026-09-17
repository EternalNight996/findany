# -*- coding: utf-8 -*-
"""字段提取器：把 doc/etest-log/extract-oa3.ps1 的 ASCII 锚点移植为 Python。

锚点与 ps1 逐条对应；OA3 正文块每台出现 2 次，一律取第 1 次并记录 block_count。
JSON 期望值基准：doc/etest-log/oa3-samples.json（SN/PKID/Hash_len=4000/SHA256…）。
"""
from __future__ import annotations

import hashlib
import json
import os
import re
from typing import Dict, List, Optional

from .types import LogType, detect_log_type

# ---------- ASCII 锚点（与 extract-oa3.ps1 对齐） ----------
RE_PROJECT = re.compile(r"e-autotest_v[\d.]+")
RE_SN = re.compile(r'msg="SN:\s*([^"]+)"')
RE_OA3_RESULT = re.compile(r'msg="OA3=([^"]+)"')
RE_PK = re.compile(r'msg="Product key:\s*([^"]+)"')
RE_PKID = re.compile(r"<ProductKeyID>(\d+)</ProductKeyID>")
RE_PKSTATE = re.compile(r"<ProductKeyState>(\d+)</ProductKeyState>")
RE_HASH = re.compile(r"<HardwareHash>([^<]+)</HardwareHash>")
RE_SIGNATURE = re.compile(r'msg="SIGNATURE:\s*([^"]+)"')
RE_MSDM_LEN = re.compile(r'msg="Length:\s*(\d+)\(')
RE_MSDM_REV = re.compile(r'msg="Revision:\s*(\d+)"')          # 锚点含 msg=" 不会误命中 OEMRevision
RE_CHECKSUM = re.compile(r'msg="CheckSum:\s*(0x[0-9a-fA-F]+)"')
RE_OEMID = re.compile(r'msg="OEMID:\s*([^"]+)"')
RE_OEM_TABLE = re.compile(r'msg="OEMTableID:\s*([^"]+)"')
RE_OEM_REV = re.compile(r'msg="OEMRevision:\s*(\d+)"')
RE_CREATOR_ID = re.compile(r'msg="CreatorID:\s*([^"]+)"')
RE_CREATOR_REV = re.compile(r'msg="CreatorRev:\s*(\d+)"')
RE_PAYLOAD_TYPE = re.compile(r'msg="Type:\s*(\d+)"')
RE_PAYLOAD_LEN = re.compile(r'msg="DataLength:\s*(\d+)"')
RE_REPORT_CBR = re.compile(r'msg="Report CBR ([^"]+)"')
RE_SEND_STATION = re.compile(r'msg="Send station:([^"]+)"')
RE_BASEBOARD = re.compile(r'\(/BP\)Baseboard product\s+\S+\s+\S+\s+\\"([^"\\]+)')
RE_LINE_TS = re.compile(r"^\[([^\]]+)\]")
RE_LOG_UPLOAD = re.compile(r'msg="文件云上传\(含批次号\)=([^"]+)"')

# ---------- heg-admin-log txt.rs 同款忽略表 ----------
MAC_IGNORE = ("00-00-00-00-00-00", "88-88-88-88-87-88", "88-88-88-88-88-88", "to be filled by o.e.m.")
BURN_IGNORE = ("ERROR", "to be filled by o.e.m.", "无法", "烧录")
# e-autotest 尾部 JSON 条目 -> 标准字段（from_e_autotest 的 app_tag 分发表）
APP_TAG_MAP = {"UUID校验": "uuid", "系统SN校验": "system_sn", "板卡SN校验": "board_sn", "BIOS版本校验": "bios_version"}


def _first(pattern: re.Pattern, text: str, group: int = 1) -> str:
    m = pattern.search(text)
    return m.group(group).strip() if m else ""


def _tail_json(text: str) -> Optional[dict]:
    """尾部结构化 JSON：最后一个 R<{ ... }>R。解析失败返回 None。"""
    start = text.rfind("R<{")
    if start < 0:
        return None
    raw = text[start + 2:]
    raw = re.sub(r">R\s*$", "", raw)
    try:
        return json.loads(raw)
    except Exception:
        return None


def _data_items(payload: Optional[dict]) -> List[dict]:
    """opts.data[] 优先，根层 data 兜底（与 ps1 一致）。"""
    if not payload:
        return []
    opts = payload.get("opts") or {}
    arr = opts.get("data") or payload.get("data") or []
    return arr if isinstance(arr, list) else []


def _inject_block(lines: List[str]) -> Dict:
    """首个 OA3 inject Start..End 块：起止时间戳 + 块出现次数（去重陷阱：每台 2 次）。"""
    bs = be = -1
    for i, line in enumerate(lines):
        if bs < 0 and "OA3 inject Start" in line:
            bs = i
        elif bs >= 0 and "OA3 inject End" in line:
            be = i
            break
    out = {"inject_start_at": "", "inject_end_at": "", "oa3_block_count": 0}
    out["oa3_block_count"] = sum(1 for l in lines if "OA3 inject Start" in l)
    if bs >= 0:
        m = RE_LINE_TS.match(lines[bs])
        out["inject_start_at"] = m.group(1) if m else ""
    if be >= 0:
        m = RE_LINE_TS.match(lines[be])
        out["inject_end_at"] = m.group(1) if m else ""
    return out


def _hash_sha256(h: str) -> str:
    return hashlib.sha256(h.encode("ascii", errors="ignore")).hexdigest()


def _base_fields(path: str, text: str) -> Dict:
    """各类型共用基础字段 + 尾部 JSON 概要。"""
    payload = _tail_json(text)
    opts = (payload or {}).get("opts") or {}
    items = _data_items(payload)
    return {
        "log_file": os.path.basename(path),
        "station": os.path.basename(os.path.dirname(path)) or ".",
        "production_num": os.path.splitext(os.path.basename(path))[0],
        "project_version": _first(RE_PROJECT, text, 0) or str(opts.get("autotest_version") or ""),
        "sn": _first(RE_SN, text) or str(opts.get("lot_sn_code") or ""),
        "mo_lot_no": str(opts.get("mo_lot_no") or ""),
        "task_tag": str(opts.get("task_tag") or ""),
        "worker_no": str(opts.get("worker_no") or ""),
        "json_status": "Pass" if (payload or {}).get("status") is True else ("" if payload is None else "Fail"),
        "json_current_item": str(opts.get("current_test_item") or ""),
        "json_current_res": str(opts.get("current_test_item_res") or ""),
        "json_item_count": len(items),
        "json_items": ";".join(str(it.get("app_tag") or "") for it in items if isinstance(it, dict)),
        "log_upload_result": _first(RE_LOG_UPLOAD, text),
    }


def _oa3_item(items: List[dict]) -> Optional[dict]:
    for it in items:
        if isinstance(it, dict) and it.get("app_tag") == "OA3":
            return it
    return None


def extract_etest_oa3(path: str, text: str) -> Dict:
    """etest(OA3)：完整 OA3 字段（对照 doc/etest-log/OA3-字段清单.md）。"""
    f = _base_fields(path, text)
    lines = text.splitlines()
    f.update(_inject_block(lines))

    hashes = [m.group(1) for m in RE_HASH.finditer(text)]
    h = hashes[0] if hashes else ""
    item = _oa3_item(_data_items(_tail_json(text)))
    rt = (item or {}).get("runtime") or {}

    f.update({
        "has_oa3": bool(hashes or item),
        "oa3_result": _first(RE_OA3_RESULT, text),
        "product_key": _first(RE_PK, text),
        "product_key_id": _first(RE_PKID, text),
        "product_key_state": _first(RE_PKSTATE, text),
        "msdm_signature": _first(RE_SIGNATURE, text),
        "msdm_length": _first(RE_MSDM_LEN, text),
        "msdm_revision": _first(RE_MSDM_REV, text),
        "msdm_checksum": _first(RE_CHECKSUM, text),
        "oem_id": _first(RE_OEMID, text),
        "oem_table_id": _first(RE_OEM_TABLE, text),
        "oem_revision": _first(RE_OEM_REV, text),
        "creator_id": _first(RE_CREATOR_ID, text),
        "creator_rev": _first(RE_CREATOR_REV, text),
        "payload_type": _first(RE_PAYLOAD_TYPE, text),
        "payload_data_len": _first(RE_PAYLOAD_LEN, text),
        "report_cbr": _first(RE_REPORT_CBR, text),
        "send_station": _first(RE_SEND_STATION, text),
        "baseboard_product": _first(RE_BASEBOARD, text),
        "hash_occurrences": len(hashes),
        "hash_consistent": len(set(hashes)) <= 1 if hashes else False,
        "hardware_hash": h,
        "hardware_hash_len": len(h),
        "hardware_hash_sha256": _hash_sha256(h) if h else "",
        "hardware_hash_head": h[:16],
        "json_state": str((item or {}).get("state") or ""),
        "json_res_value": str((item or {}).get("res_value") or ""),
        "json_err_count": (item or {}).get("err_count", ""),
        "json_ok_count": (item or {}).get("ok_count", ""),
        "json_runtime_secs": rt.get("secs", "") if isinstance(rt, dict) else "",
        "json_runtime_nanos": rt.get("nanos", "") if isinstance(rt, dict) else "",
        "oa3_log_path": str((item or {}).get("value") or ""),
    })
    _map_app_tags(f, _data_items(_tail_json(text)))      # 校验类/MAC获取同源分发
    f["oa3_key"] = f.get("product_key", "")              # 列别名对齐 heg-admin-log
    f["oa3_id"] = f.get("product_key_id", "")
    # baseboard 兜底：主板型号校验结果行 / JSON 条目
    if not f["baseboard_product"]:
        m = re.search(r'msg="主板型号校验=([^"]+)"', text)
        if m:
            f["baseboard_product"] = m.group(1).strip()
    if not f["baseboard_product"]:
        for it in _data_items(_tail_json(text)):
            if isinstance(it, dict) and it.get("app_tag") == "主板型号校验":
                f["baseboard_product"] = str(it.get("res_value") or it.get("value") or "")
                break
    return f


def extract_generic(path: str, text: str) -> Dict:
    """etest / e-autotest（无 OA3）：尾部 JSON 概要 + 基础字段 + app_tag 分发
    （UUID/系统SN/板卡SN/BIOS版本/激活码/MAC，同 heg-admin-log from_e_autotest）。"""
    f = _base_fields(path, text)
    f["has_oa3"] = False
    _map_app_tags(f, _data_items(_tail_json(text)))
    return f


# ---------- 海格旧测试 2/3（移植 heg-admin-log from_heg2/from_heg3） ----------

def _trim_list(dst: List[str], line: str, pat: str, ignores=BURN_IGNORE) -> None:
    """取 pat 右侧值；命中忽略词跳过；去重（同 trim_data_list）。"""
    line = line.rstrip("\n ")
    for ig in ignores:
        if ig in line:
            return
    res = line.split(pat, 1)[1].strip() if pat in line else ""
    if res and res not in dst:
        dst.append(res)


def _xml_between(line: str, start: str, end: str):
    """取 <start>..</end> 中间值。注意：上游 trim_data_list2 的 contains 条件写反
    （只在已包含时 push），此处按意图修复为去重后 push。"""
    i = line.find(start)
    j = line.find(end)
    if 0 <= i < j:
        mid = line[i + len(start):j].rstrip(" ")
        return mid or None
    return None


def _mac_class(name: str):
    """接口名 -> lan/wifilan/bluetooth；虚拟网卡 None（同参考分类）。"""
    if "vEthernet" in name or "虚拟" in name:
        return None
    if "Ethernet" in name or "以太网" in name:
        return "lan"
    if "WLAN" in name or "Wi-Fi" in name or "无线" in name:
        return "wifilan"
    if "Bluetooth" in name or "蓝牙" in name:
        return "bluetooth"
    return None


def _add_mac(macs: Dict, cls, mac: str, keep_dash: bool) -> None:
    mac = str(mac or "").strip()
    if not keep_dash:
        mac = mac.replace("-", "")
    if not mac or mac.lower() in MAC_IGNORE:
        return
    if cls and mac not in macs[cls]:
        macs[cls].append(mac)


def _empty_macs() -> Dict:
    return {"lan": [], "wifilan": [], "bluetooth": []}


_HEG_AT_ANCHORS = (("@OS激活码=", "os_key"), ("@UUID=", "uuid"), ("@BIOS_SN=", "system_sn"),
                   ("@BOARD_SN=", "board_sn"), ("@BIOS版本=", "bios_version"))


def extract_heg3(path: str, text: str) -> Dict:
    """海格旧测试3（IFT/CLEAN/BURN/FFT/BATTERY/BFT 前缀）：@锚点 + <ProductKey> + @网络MAC JSON。"""
    f = _base_fields(path, text)
    f["production_num"] = os.path.splitext(os.path.basename(path))[0]
    f["has_oa3"] = False
    vals = {k: [] for _, k in _HEG_AT_ANCHORS}
    vals["oa3_key"], vals["oa3_id"] = [], []
    macs = _empty_macs()
    for line in text.splitlines():
        if "<ProductKeyID>" in line:
            mid = _xml_between(line, "<ProductKeyID>", "</ProductKeyID>")
            if mid and mid not in vals["oa3_id"]:
                vals["oa3_id"].append(mid)
            continue
        if "<ProductKey>" in line:
            mid = _xml_between(line, "<ProductKey>", "</ProductKey>")
            if mid and mid not in vals["oa3_key"]:
                vals["oa3_key"].append(mid)
            continue
        if "@网络MAC=[{" in line:
            try:
                lst = json.loads(line.split("@网络MAC=", 1)[1])
            except Exception:
                lst = None
            for info in lst or []:
                if not isinstance(info, dict):
                    continue
                _add_mac(macs, _mac_class(str(info.get("interface", ""))), info.get("mac", ""), keep_dash=True)
            continue
        for anchor, key in _HEG_AT_ANCHORS:
            if anchor in line:
                _trim_list(vals[key], line, anchor)
                break
    f["oa3_result"] = ""
    f.update({k: v[-1] if v else "" for k, v in vals.items()})       # 同 Sigle::last：取最后一次
    f.update({k: ";".join(v) for k, v in macs.items()})
    return f


def extract_heg2(path: str, text: str) -> Dict:
    """海格旧测试2（IFT-START/SN 前缀）：@锚点 + 多行「接口」块（MAC 在接口行后第 3 行）。"""
    f = _base_fields(path, text)
    f["production_num"] = os.path.splitext(os.path.basename(path))[0]
    f["has_oa3"] = False
    vals = {k: [] for _, k in _HEG_AT_ANCHORS}
    macs = _empty_macs()
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if "@网络MAC=" in line and ("接口" in line or (i + 1 < len(lines) and "接口" in lines[i + 1])):
            for j in range(i, len(lines)):
                lj = lines[j]
                if "接口" not in lj:
                    continue
                if "虚拟" in lj or "vEthernet" in lj:
                    continue
                cls = _mac_class(lj)
                if cls and j + 3 < len(lines):
                    mac = lines[j + 3].lstrip(" ")
                    if mac.startswith("MAC地址: "):
                        mac = mac[len("MAC地址: "):]
                    _add_mac(macs, cls, mac, keep_dash=True)
            continue
        for anchor, key in _HEG_AT_ANCHORS:
            if anchor in line:
                _trim_list(vals[key], line, anchor)
                break
    f.update({k: v[-1] if v else "" for k, v in vals.items()})
    f.update({k: ";".join(v) for k, v in macs.items()})
    return f


def _map_app_tags(f: Dict, items: List[dict]) -> None:
    """e-autotest 尾部 JSON 条目分发（同 from_e_autotest）：校验类/激活类/MAC获取。"""
    macs = _empty_macs()
    for it in items:
        if not isinstance(it, dict):
            continue
        tag = str(it.get("app_tag", "") or "")
        res = str(it.get("res_value", "") or "").strip()
        key = APP_TAG_MAP.get(tag)
        if key and res:
            f[key] = res                                   # 多次出现取最后（同 Sigle::last）
        elif ("系统激活" in tag or "自动化激活" in tag) and res:
            f["os_key"] = res
        elif "MAC获取" in tag and res:
            js = res.split("=", 1)[1] if "=" in res else ""
            try:
                lst = json.loads(js)
            except Exception:
                lst = None
            for info in lst or []:
                if not isinstance(info, dict):
                    continue
                cls = _mac_class(str(info.get("friendly_name", "")))
                if cls is None:
                    # 参考的 if_type 兜底：以太网类->lan，Wireless80211->wifilan
                    t = str(info.get("if_type", "") or "").lower()
                    if t.startswith("ethernet") or t == "gigabitethernet" or "fastethernet" in t:
                        cls = "lan"
                    elif t == "wireless80211":
                        cls = "wifilan"
                _add_mac(macs, cls, info.get("mac_addr", ""), keep_dash=False)
    f["lan"] = ";".join(macs["lan"])
    f["wifilan"] = ";".join(macs["wifilan"])
    f["bluetooth"] = ";".join(macs["bluetooth"])


def extract_unknown(path: str, text: str) -> Dict:
    return {
        "log_file": os.path.basename(path),
        "station": os.path.basename(os.path.dirname(path)) or ".",
        "has_oa3": False,
        "project_version": _first(RE_PROJECT, text),
    }


_EXTRACTORS = {
    LogType.ETEST_OA3: extract_etest_oa3,
    LogType.ETEST: extract_generic,
    LogType.EAUTOTEST: extract_generic,
    LogType.HEG_AUTOTEST2: extract_heg2,
    LogType.HEG_AUTOTEST3: extract_heg3,
    LogType.UNKNOWN: extract_unknown,
}


def extract(path: str, text: str, log_type: Optional[LogType] = None) -> Dict:
    """按类型提取字段；log_type 缺省时自动判型。返回 fields 含 detected_type。"""
    t = log_type or detect_log_type(path, text)
    f = _EXTRACTORS[t](path, text)
    f["detected_type"] = t.value
    return f


# ---------- 读文件（编码容错，与 sonar.scanner 降级序一致） ----------

def read_text(path: str, max_mb: float = 20.0) -> str:
    """读取全文；超限抛 ValueError；编码逐级降级，最终 errors=replace 兜底。"""
    if os.path.getsize(path) > max_mb * 1024 * 1024:
        raise ValueError("too_large")
    for enc in ("utf-8-sig", "utf-8", "gbk", "utf-16"):
        try:
            with open(path, "r", encoding=enc, errors="strict") as fh:
                return fh.read()
        except (UnicodeDecodeError, LookupError):
            continue
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return fh.read()
