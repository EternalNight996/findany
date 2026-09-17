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
    """etest / e-autotest（无 OA3）：尾部 JSON 概要 + 基础字段。"""
    f = _base_fields(path, text)
    f["has_oa3"] = False
    return f


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
