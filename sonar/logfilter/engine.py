# -*- coding: utf-8 -*-
"""筛选引擎：目录遍历 -> 判型 -> 字段提取 -> （可选）逐台回传 -> 批次产物。

不依赖 Qt，可独立复用/测试。两阶段进度：提取(0~100) 后 回传(0~N)。
"""
from __future__ import annotations

import os
import time
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional

from . import report
from .extractors import extract, read_text
from .types import LogType, detect_log_type
from .uploader import ST_CONFLICT, ST_DRY_RUN, ST_FAIL, ST_OK, UploadProfile, UploadResult, resolve_cli, run_upload
from ..exporter import make_batch_dir


@dataclass
class FilterRunCfg:
    root_dir: str = ""
    out_dir: str = ""                    # 批次输出根
    log_type: str = LogType.AUTO.value   # auto / etest(OA3) / etest / e-autotest
    recursive: bool = True
    extensions: List[str] = field(default_factory=lambda: ["log"])
    encoding: str = "auto"               # GUI「编码」共享项（auto/utf-8/gbk/utf-16/ascii）
    max_file_mb: float = 20.0
    threads: int = 8
    keep_logs: bool = True               # 提取成功日志留存
    upload_enabled: bool = False
    upload_types: List[str] = field(default_factory=lambda: ["etest(OA3)"])  # 参与回传的判型（定版②A：默认仅 OA3）
    dry_run: bool = True
    profile: UploadProfile = field(default_factory=UploadProfile)
    app_dir: str = ""                    # 用于 CLI 默认路径解析；空=项目根兜底
    file_list: Optional[List[str]] = None   # 指定文件清单（SN关联/单文件方案）；None=走目录遍历


@dataclass
class FilterSummary:
    total: int = 0
    extracted: int = 0
    unknown: int = 0
    skipped: int = 0
    upload_ok: int = 0
    upload_conflict: int = 0
    upload_fail: int = 0
    upload_dry: int = 0
    upload_skip: int = 0
    elapsed: float = 0.0
    batch_dir: str = ""
    excel_path: str = ""
    audit_path: str = ""


class FilterEngine:
    """日志筛选 + 回传编排。on_progress/on_upload/on_log 回调驱动 GUI。"""

    def __init__(self, cfg: FilterRunCfg,
                 on_progress: Callable[[Dict], None] | None = None,
                 on_upload: Callable[[Dict], None] | None = None,
                 on_log: Callable[[str, str], None] | None = None):
        self.cfg = cfg
        if not cfg.app_dir:
            # sonar/logfilter/engine.py -> 项目根（frozen 下 GUI 已显式传 APP_DIR）
            cfg.app_dir = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
        self._on_progress = on_progress or (lambda d: None)
        self._on_upload = on_upload or (lambda d: None)
        self._on_log = on_log or (lambda lvl, msg: None)
        self._cancel = False

    def cancel(self):
        self._cancel = True

    # ---------- 遍历 ----------
    def _walk(self) -> List[str]:
        exts = {e.lower().lstrip(".") for e in self.cfg.extensions if e and e.strip()}
        out: List[str] = []
        root = self.cfg.root_dir
        if self.cfg.recursive:
            for cur, dirs, files in os.walk(root):
                dirs[:] = [d for d in dirs if not d.startswith(".")]
                for f in files:
                    if f.startswith("."):
                        continue
                    if exts and ("." not in f or f.rsplit(".", 1)[1].lower() not in exts):
                        continue
                    out.append(os.path.join(cur, f))
        else:
            try:
                for f in os.listdir(root):
                    p = os.path.join(root, f)
                    if os.path.isfile(p) and not f.startswith("."):
                        if exts and ("." not in f or f.rsplit(".", 1)[1].lower() not in exts):
                            continue
                        out.append(p)
            except Exception:
                pass
        out_abs = os.path.abspath(self.cfg.out_dir) if self.cfg.out_dir else ""
        if out_abs:
            out = [p for p in out if not self._under(p, out_abs)]
        return sorted(out)

    @staticmethod
    def _under(path: str, base: str) -> bool:
        try:
            return os.path.commonpath([os.path.abspath(path), base]) == base
        except ValueError:
            return False

    # ---------- 提取 ----------
    def _extract_one(self, path: str, root: str) -> Dict:
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        item: Dict = {
            "abs_path": path, "rel_path": rel,
            "dir_name": os.path.dirname(rel) or ".",
            "filename": os.path.basename(path),
            "detected_type": "", "extract_ok": False, "error": "",
            "upload_state": "", "upload_code": "", "request_id": "",
            "resp_status": "", "upload_error": "", "upload_attempts": "", "upload_elapsed": "",
        }
        try:
            st = os.stat(path)
            item["size"] = st.st_size
            item["mtime_str"] = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(st.st_mtime))
            text = read_text(path, self.cfg.max_file_mb, self.cfg.encoding)
        except ValueError:
            item["error"] = "超大/超限跳过"
            return item
        except Exception as e:
            item["error"] = f"读取失败: {e}"
            return item

        forced = None
        if self.cfg.log_type and self.cfg.log_type != LogType.AUTO.value:
            try:
                forced = LogType(self.cfg.log_type)
            except ValueError:
                forced = None
        detected = detect_log_type(path, text)
        item["detected_type"] = (forced or detected).value
        try:
            fields = extract(path, text, forced or detected)
            item["fields"] = fields
            item.update({k: fields.get(k, "") for k in
                         ("log_file", "station", "sn", "oa3_result", "product_key_id",
                          "product_key_state", "hardware_hash_len", "hardware_hash_sha256",
                          "inject_start_at", "inject_end_at", "product_key",
                          "baseboard_product", "mo_lot_no", "task_tag", "json_state",
                          "json_res_value", "project_version",
                          "production_num", "system_sn", "board_sn", "uuid", "bios_version",
                          "os_key", "lan", "wifilan", "bluetooth", "oa3_key", "oa3_id")})
            item["extract_ok"] = True
        except Exception:
            item["error"] = "提取异常: " + traceback.format_exc(limit=1).strip().splitlines()[-1]
        return item

    # ---------- 主流程 ----------
    def run(self):
        cfg = self.cfg
        t0 = time.time()
        s = FilterSummary()
        files = list(cfg.file_list) if cfg.file_list is not None else self._walk()
        s.total = len(files)
        self._on_log("info", f"日志筛选：{cfg.root_dir} 共 {len(files)} 个文件，类型={cfg.log_type or 'auto'}，"
                             f"回传={'开' if cfg.upload_enabled else '关'}"
                             + ("（dry-run）" if cfg.upload_enabled and cfg.dry_run else ""))
        self._on_progress({"phase": "extract", "done": 0, "total": len(files), "pct": 0.0})

        items: List[Dict] = []
        if files:
            with ThreadPoolExecutor(max_workers=max(1, cfg.threads)) as ex:
                futs = {ex.submit(self._extract_one, p, cfg.root_dir): p for p in files}
                for i, fut in enumerate(as_completed(futs), start=1):
                    if self._cancel:
                        break
                    it = fut.result()
                    items.append(it)
                    if it["extract_ok"]:
                        s.extracted += 1
                        if it["detected_type"] == LogType.UNKNOWN.value:
                            s.unknown += 1
                    else:
                        s.skipped += 1
                        self._on_log("warn", f"{it['rel_path']}：{it['error']}")
                    self._on_progress({"phase": "extract", "done": i, "total": len(files),
                                       "pct": i / len(files) * 100.0})
        items.sort(key=lambda d: d["rel_path"])

        # ---------- 回传（SOP：逐台串行，一次一台） ----------
        if cfg.upload_enabled and not self._cancel:
            cli = resolve_cli(cfg.profile.cli_path, cfg.app_dir)
            profile = cfg.profile
            profile.cli_path = cli
            if not cli:
                self._on_log("err", "回传已启用但找不到 CLI（intunehelper_cli.exe），请在配置中选择")
            else:
                self._on_log("info", f"回传 CLI：{cli}" + ("  [dry-run]" if cfg.dry_run else "")
                             + f"  回传类型：{','.join(cfg.upload_types)}")
                want_types = set(cfg.upload_types)
                targets = [it for it in items if it["extract_ok"] and it["detected_type"] in want_types]
                total = len(targets)
                for i, it in enumerate(targets, start=1):
                    if self._cancel:
                        break
                    fields = it.get("fields") or {}
                    res: UploadResult = run_upload(profile, fields, dry_run=cfg.dry_run, on_log=self._on_log)
                    it.update({
                        "upload_state": {"ok": "成功", "conflict": "冲突(人工)", "fail": "失败",
                                         "dry_run": "dry-run"}.get(res.status, res.status),
                        "upload_code": "" if res.exit_code is None else str(res.exit_code),
                        "request_id": res.request_id, "resp_status": res.resp_status,
                        "upload_error": res.error, "upload_attempts": res.attempts or "",
                        "upload_elapsed": res.elapsed,
                    })
                    if res.status == ST_OK:
                        s.upload_ok += 1
                    elif res.status == ST_CONFLICT:
                        s.upload_conflict += 1
                    elif res.status == ST_FAIL:
                        s.upload_fail += 1
                    else:  # dry_run
                        s.upload_dry += 1
                    if res.error:
                        lvl = "warn" if res.status in (ST_CONFLICT, ST_DRY_RUN) else "err"
                        self._on_log(lvl, f"[{i}/{total}] {it['rel_path']} 回传：{res.error}")
                    else:
                        self._on_log("ok", f"[{i}/{total}] {it['rel_path']} 回传：{res.resp_status or 'dry-run'}"
                                           f"  request_id={res.request_id or '-'}")
                    self._on_upload({"index": i, "total": total, "sn": it.get("sn", ""),
                                     "status": res.status, "pct": i / total * 100.0 if total else 0.0})
                s.upload_skip = sum(1 for it in targets if not it.get("upload_state"))
        elif cfg.upload_enabled and self._cancel:
            self._on_log("warn", "已取消，跳过回传")

        # ---------- 产物 ----------
        if not self._cancel and items:
            try:
                batch = make_batch_dir(cfg.out_dir or os.getcwd())
                s.batch_dir = batch.path
                rows = [dict(it) for it in items]
                for r in rows:
                    r.setdefault("extract_state", "成功" if r.get("extract_ok") else "失败")
                summary_rows = [
                    ("扫描目录", cfg.root_dir), ("筛选类型", cfg.log_type or "auto"),
                    ("文件总数", s.total), ("提取成功", s.extracted), ("未知类型", s.unknown),
                    ("跳过", s.skipped),
                    ("回传开关", ("开" + ("（dry-run）" if cfg.dry_run else "")) if cfg.upload_enabled else "关"),
                    ("回传成功", s.upload_ok), ("回传冲突", s.upload_conflict), ("回传失败", s.upload_fail),
                    ("耗时(秒)", round(time.time() - t0, 2)),
                    ("导出时间", time.strftime("%Y-%m-%d %H:%M:%S")),
                ]
                s.excel_path = report.export_filter_excel(os.path.join(batch.path, "filter_result.xlsx"),
                                                          rows, summary_rows)
                s.audit_path = report.write_upload_audit(os.path.join(batch.path, "upload-result.csv"), rows)
                kept = report.copy_logs(items, batch.path) if cfg.keep_logs else 0
                self._on_log("ok", f"产物：{batch.path}  （Excel {os.path.basename(s.excel_path)}"
                                   f" + 审计 upload-result.csv + 留存 {kept} 份日志）")
            except Exception as e:
                self._on_log("err", "产物导出失败：" + str(e))

        s.elapsed = round(time.time() - t0, 2)
        self._on_progress({"phase": "done", "done": s.total, "total": s.total, "pct": 100.0})
        self._on_log("ok", f"筛选完成：提取 {s.extracted}/{s.total}，未知 {s.unknown}，跳过 {s.skipped}，"
                           f"回传 成功 {s.upload_ok} / 冲突 {s.upload_conflict} / 失败 {s.upload_fail}"
                           + (f" / dry-run {s.upload_dry}" if s.upload_dry else "") + f"，耗时 {s.elapsed}s")
        return items, s
