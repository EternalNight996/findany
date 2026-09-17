# -*- coding: utf-8 -*-
"""通用 CLI 回传：子进程调第三方 CLI（默认 intunehelper_cli.exe）逐台上传并判结果。

判定规则移植 etest-core check_data 思路（定版 ④A）：
  退出码 ∈ success_codes 且 stdout JSON status ∈ status_ok  -> ok（绿）
  退出码 ∈ conflict_codes                                   -> conflict（黄，转人工）
  退出码 ∈ retry_codes 仅重试；其余 / 判定失败               -> fail（红）
所有要素（CLI 路径/参数模板/stdin/超时/重试/码表）均可配置，持久化在 config.json。
"""
from __future__ import annotations

import json
import os
import shlex
import subprocess
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Tuple

# 每台上传结果状态
ST_OK = "ok"
ST_CONFLICT = "conflict"
ST_FAIL = "fail"
ST_DRY_RUN = "dry_run"

REQUIRED_FIELDS = ("serial_number", "product_key_id", "hardware_hash", "baseboard_product")


def default_field_map() -> Dict[str, str]:
    """payload 键 <- 提取字段名（etest(OA3) 提取器字段）。"""
    return {
        "serial_number": "sn",
        "product_key_id": "product_key_id",
        "hardware_hash": "hardware_hash",
        "baseboard_product": "baseboard_product",
    }


@dataclass
class UploadProfile:
    """一次回传的完整配置（GUI 可改，config.json 持久化）。"""
    cli_path: str = ""                 # 空 = 程序目录下 intunehelper_cli.exe
    args: str = "upload --stdin --secret-key ~secret_key~"   # 占位符 ~key~ 仿 etest rkey
    use_stdin: bool = True             # False 时 payload 经 ~payload~ 传参
    timeout_sec: float = 60.0
    max_retries: int = 3               # 仅对 retry_codes 生效
    retry_delay: float = 1.0           # 退避基数：delay * 2^n
    secret_key: str = ""
    success_codes: Tuple[int, ...] = (0,)
    conflict_codes: Tuple[int, ...] = (12,)
    retry_codes: Tuple[int, ...] = (21,)
    status_ok: Tuple[str, ...] = ("accepted", "duplicate_accepted")
    field_map: Dict[str, str] = field(default_factory=default_field_map)


@dataclass
class UploadResult:
    status: str = ST_FAIL              # ok / conflict / fail / dry_run
    exit_code: Optional[int] = None
    resp_status: str = ""              # stdout JSON 的 status
    request_id: str = ""
    serial_number: str = ""
    attempts: int = 0
    elapsed: float = 0.0
    payload_json: str = ""             # dry-run / 校验用；审计文件不落它（SOP 第 6 条）
    error: str = ""
    stdout: str = ""


def build_payload(fields: Dict, field_map: Dict[str, str]) -> Dict[str, str]:
    """按 field_map 组 payload；全部转字符串。"""
    out: Dict[str, str] = {}
    for key, src in field_map.items():
        out[key] = str(fields.get(src, "") or "").strip()
    return out


def missing_fields(payload: Dict[str, str]) -> List[str]:
    return [k for k in REQUIRED_FIELDS if not payload.get(k)]


def render_args(args: str, profile: UploadProfile, payload_json: str, fields: Dict) -> List[str]:
    """把 ~key~ 占位符渲染成实际参数。secret_key/payload 内置，其余查提取字段。"""
    mapping = {"secret_key": profile.secret_key, "payload": payload_json}
    mapping.update({k: str(v) for k, v in fields.items()})
    tokens = shlex.split(args, posix=False) if os.name == "nt" else shlex.split(args)

    def render(tok: str) -> str:
        if len(tok) >= 3 and tok.startswith("~") and tok.endswith("~"):
            return mapping.get(tok[1:-1], tok)
        return tok

    out = [render(t).strip('"') for t in tokens]
    return out


def resolve_cli(cli_path: str, app_dir: str = "") -> str:
    """CLI 路径解析：显式路径 > 程序目录 > doc/devicehashupload（开发布局兜底）。"""
    if cli_path and os.path.isfile(cli_path):
        return cli_path
    cands = []
    if app_dir:
        cands.append(os.path.join(app_dir, "intunehelper_cli.exe"))
        cands.append(os.path.join(app_dir, "doc", "devicehashupload", "intunehelper_cli.exe"))
    for c in cands:
        if os.path.isfile(c):
            return c
    return cli_path or ""


def _default_runner(cmd: List[str], stdin_bytes: Optional[bytes], use_stdin: bool,
                    timeout: float) -> Tuple[Optional[int], str, str]:
    """真实子进程调用。stdin 写完必须关闭（CLI 收到 EOF 才上传，SOP 第 4 节）。"""
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE if use_stdin else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        out, err = proc.communicate(input=stdin_bytes if use_stdin else None, timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            proc.kill()
            proc.communicate(timeout=5)
        except Exception:
            pass
        return None, "", "timeout"
    return (
        proc.returncode,
        (out or b"").decode("utf-8", errors="replace"),
        (err or b"").decode("utf-8", errors="replace"),
    )


def _parse_stdout_json(stdout: str) -> Optional[dict]:
    """取 stdout 里最后一行 JSON（SOP：成功时单行结果 JSON）。"""
    for line in reversed([l for l in stdout.splitlines() if l.strip()]):
        line = line.strip()
        if line.startswith("{"):
            try:
                v = json.loads(line)
                return v if isinstance(v, dict) else None
            except Exception:
                continue
    return None


def run_upload(profile: UploadProfile, fields: Dict, dry_run: bool = False,
               runner=None, on_log: Optional[Callable[[str, str], None]] = None) -> UploadResult:
    """上传一台。runner 可注入（测试用）；dry_run 只组包与校验，不打网不落地。"""
    log = on_log or (lambda lvl, msg: None)
    payload = build_payload(fields, profile.field_map)
    miss = missing_fields(payload)
    payload_json = json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    res = UploadResult(payload_json=payload_json, serial_number=payload.get("serial_number", ""))
    if miss:
        res.status = ST_FAIL
        res.error = "字段不全: " + ",".join(miss)
        return res

    if dry_run:
        res.status = ST_DRY_RUN
        return res

    runner = runner or _default_runner
    cli = profile.cli_path
    if runner is _default_runner and (not cli or not os.path.isfile(cli)):
        res.error = f"CLI 不存在: {cli or '(空)'}"
        return res
    cmd = [cli] + render_args(profile.args, profile, payload_json, fields)
    stdin_bytes = payload_json.encode("utf-8")  # UTF-8 无 BOM（SOP 4.1）

    t0 = time.time()
    max_try = max(1, profile.max_retries + 1)
    for attempt in range(1, max_try + 1):
        res.attempts = attempt
        try:
            rc, stdout, stderr = runner(cmd, stdin_bytes, profile.use_stdin, profile.timeout_sec)
        except Exception as e:  # 启动失败等
            res.error = f"CLI 调用异常: {e}"
            res.elapsed = round(time.time() - t0, 2)
            return res
        res.exit_code, res.stdout = rc, stdout
        resp = _parse_stdout_json(stdout)
        res.resp_status = str(resp.get("status") or "") if resp else ""
        res.request_id = str(resp.get("request_id") or "") if resp else ""

        if rc in profile.success_codes:
            # 定版 ④A：双确认。退出码 0 且 stdout status 命中才 ok
            if resp is None:
                res.status = ST_FAIL
                res.error = "退出码 0 但 stdout 无结果 JSON"
            elif res.resp_status in profile.status_ok:
                res.status = ST_OK
            else:
                res.status = ST_FAIL
                res.error = f"退出码 0 但 status={res.resp_status or '(空)'}"
            break
        if rc in profile.conflict_codes:
            res.status = ST_CONFLICT
            res.error = f"记录冲突(退出码 {rc})，转人工：保留 request_id"
            break
        if rc in profile.retry_codes and attempt < max_try:
            delay = profile.retry_delay * (2 ** (attempt - 1))
            log("warn", f"回传暂不可用(退出码 {rc})，{delay:.0f}s 后第 {attempt + 1} 次尝试")
            if delay > 0:
                time.sleep(delay)
            continue
        res.status = ST_FAIL
        res.error = f"退出码 {rc}" + (f"：{stderr.strip()[:200]}" if stderr.strip() else "")
        break
    res.elapsed = round(time.time() - t0, 2)
    return res
