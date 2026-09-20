# -*- coding: utf-8 -*-
"""扫描核心：递归遍历、并发读文本、包含/不含判定、行号统计。

不依赖 Qt，可独立复用/测试。Qt 侧通过回调获取进度与日志。
"""
from __future__ import annotations

import os
import time
import threading
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Tuple

from .config import SearchConfig


@dataclass
class ScanItem:
    """单个文件的扫描结果。"""
    dir_name: str = ""          # 所在目录名
    filename: str = ""          # 文件名
    rel_path: str = ""          # 相对扫描根目录的路径
    abs_path: str = ""          # 绝对路径
    hit: bool = False           # 是否命中（匹配期望模式）
    keyword: str = ""
    hit_lines: List[int] = field(default_factory=list)   # 命中行号
    hit_line_text: str = ""        # 首条命中行内容（原始大小写）
    hit_count: int = 0
    size: int = 0
    mtime: float = 0.0
    encoding: str = ""
    skipped: str = ""           # 若跳过，原因（如 "二进制/超大"）

    @property
    def mtime_str(self) -> str:
        return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(self.mtime))

    @property
    def size_str(self) -> str:
        v = float(self.size)
        for unit in ("B", "KB", "MB", "GB"):
            if v < 1024 or unit == "GB":
                return f"{v:.1f} {unit}"
            v /= 1024
        return f"{self.size} B"


@dataclass
class ScanSummary:
    total_files: int = 0
    scanned: int = 0
    hit: int = 0
    miss: int = 0
    skipped: int = 0
    elapsed: float = 0.0
    dir: str = ""


# ---------- 编码读取 ----------

def detect_encoding(path: str, max_bytes: int = 8192) -> str:
    """用 chardet 探测文件编码，失败降级 utf-8。"""
    try:
        import chardet
        with open(path, "rb") as f:
            raw = f.read(max_bytes)
        if not raw:
            return "utf-8"
        res = chardet.detect(raw)
        enc = (res.get("encoding") or "utf-8").lower()
        for alias, norm in {
            "gb2312": "gbk", "gb18030": "gbk", "big5": "big5",
            "utf-8-sig": "utf-8-sig", "utf-16": "utf-16", "utf-16le": "utf-16",
        }.items():
            if enc.startswith(alias):
                return norm
        return enc
    except Exception:
        return "utf-8"


def _read_lines(path: str, encoding: str, case_sensitive: bool):
    """逐行读取，yield (行号, 行文本, 实际使用编码)。编码无效时尝试降级。"""
    encodings = [encoding]
    if encoding == "auto":
        encodings = ["utf-8-sig", "utf-8", "gbk", "latin-1"]
    elif encoding == "utf-8":
        encodings = ["utf-8-sig", "utf-8", "gbk"]
    elif encoding == "gbk":
        encodings = ["gbk", "gb2312", "utf-8"]
    elif encoding == "utf-16":
        encodings = ["utf-16", "utf-8-sig", "utf-8"]
    elif encoding == "ascii":
        encodings = ["ascii", "latin-1", "utf-8"]

    for enc in encodings:
        try:
            with open(path, "r", encoding=enc, errors="strict") as f:
                for ln, line in enumerate(f, start=1):
                    yield ln, line, enc
            return
        except (UnicodeDecodeError, LookupError):
            continue
    # 全部失败：宽容解码
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for ln, line in enumerate(f, start=1):
            yield ln, line, "utf-8(replace)"


# ---------- 文件遍历 ----------

def walk_files(root: str, extensions: List[str], recursive: bool) -> List[str]:
    """收集待扫描文件路径（按扩展名过滤，空扩展名列表=全部）。"""
    exts = {e.lower().lstrip(".") for e in extensions if e and e.strip()}
    result: List[str] = []
    if recursive:
        for cur, dirs, files in os.walk(root):
            dirs[:] = [d for d in dirs if not d.startswith(".")]
            for f in files:
                if f.startswith("."):
                    continue
                if exts and ("." not in f or f.rsplit(".", 1)[1].lower() not in exts):
                    continue
                result.append(os.path.join(cur, f))
    else:
        try:
            for f in os.listdir(root):
                p = os.path.join(root, f)
                if os.path.isfile(p) and not f.startswith("."):
                    if exts and ("." not in f or f.rsplit(".", 1)[1].lower() not in exts):
                        continue
                    result.append(p)
        except Exception:
            pass
    return result


# ---------- 单个文件匹配 ----------

def match_one(path: str, cfg: SearchConfig, root: str) -> Optional[ScanItem]:
    """扫描单个文件，返回 ScanItem。无法读取/跳过时仍返回带 skipped 的 item。"""
    try:
        st = os.stat(path)
    except OSError:
        return None
    size = st.st_size
    rel = os.path.relpath(path, root)
    rel = rel.replace(os.sep, "/")
    dir_name = os.path.dirname(rel)
    if not dir_name:
        dir_name = "."
    item = ScanItem(
        dir_name=dir_name,
        filename=os.path.basename(path),
        rel_path=rel,
        abs_path=path,
        size=size,
        mtime=st.st_mtime,
    )
    # 二进制/超大判定
    if size > cfg.max_file_mb * 1024 * 1024:
        item.skipped = "too_large"
        return item
    try:
        with open(path, "rb") as f:
            head = f.read(8192)
    except OSError:
        item.skipped = "io_error"
        return item
    if head and b"\x00" in head:
        # 含零字节：仅当显式指定 UTF-16 或带 UTF-16 BOM 时视为文本，否则判定为二进制
        is_utf16 = cfg.encoding in ("utf-16", "utf-16le", "utf-16be")
        if head[:2] in (b"\xff\xfe", b"\xfe\xff"):
            is_utf16 = True
        if not is_utf16:
            item.skipped = "binary"
            return item
        item.encoding = cfg.encoding if cfg.encoding != "auto" else "utf-16"
    else:
        item.encoding = cfg.encoding if cfg.encoding != "auto" else detect_encoding(path)

    keyword = cfg.keyword
    needle = keyword.lower() if not cfg.case_sensitive else keyword

    hit_lines: List[int] = []
    hit_text = ""
    found_any = False
    enc_used = item.encoding
    try:
        for ln, line, enc in _read_lines(
                path, "auto" if cfg.encoding == "auto" else cfg.encoding, cfg.case_sensitive):
            enc_used = enc
            line_cmp = line.lower() if not cfg.case_sensitive else line
            if needle in line_cmp:
                hit_lines.append(ln)
                if not hit_text:
                    hit_text = line.strip()[:200]
                found_any = True
    except Exception:
        item.skipped = "read_error"
        return item

    item.encoding = enc_used
    item.hit_lines = hit_lines
    item.hit_line_text = hit_text
    item.hit_count = len(hit_lines)

    if cfg.mode == "inc":
        item.hit = found_any
    else:  # exc：命中 = 文件不含关键字
        item.hit = (not found_any)
    return item


# ---------- 引擎（线程池调度） ----------

class ScanEngine:
    """协调目录遍历与并发文件扫描。"""

    def __init__(self, cfg: SearchConfig, on_progress: Callable[[Dict], None] | None = None,
                 on_log: Callable[[str, str], None] | None = None):
        self.cfg = cfg
        self._on_progress = on_progress or (lambda d: None)
        self._on_log = on_log or (lambda lvl, msg: None)
        self._cancel = False
        self._lock = threading.Lock()

    def cancel(self):
        self._cancel = True

    def scan(self) -> Tuple[List[ScanItem], ScanSummary]:
        cfg = self.cfg
        t0 = time.time()
        files = [cfg.scan_file] if cfg.scan_file else walk_files(cfg.root_dir, cfg.extensions, cfg.recursive)
        # 排除输出目录（若输出在扫描根内，避免重复扫自己产物）
        out_abs = os.path.abspath(cfg.out_dir) if cfg.out_dir else ""
        if out_abs:
            files = [p for p in files if not self._under(p, out_abs)]
        total = len(files)
        summary = ScanSummary(total_files=total, dir=cfg.root_dir)
        items: List[ScanItem] = []

        self._on_log("info", "%s，共 %d 个文件，线程 %d，关键字「%s」，模式：%s" % (
            ("扫描文件：%s" % cfg.scan_file) if cfg.scan_file else ("扫描目录：%s" % cfg.root_dir),
            total, cfg.threads, cfg.keyword,
            "包含" if cfg.mode == "inc" else "不包含"))
        self._progress(0, total, 0, 0, 0)

        if total == 0:
            summary.elapsed = round(time.time() - t0, 2)
            self._on_log("warn", "未找到符合扩展名的文件")
            return items, summary

        if self._cancel:
            self._on_log("warn", "扫描已被取消")
            return items, summary

        def worker(path: str) -> Optional[ScanItem]:
            if self._cancel:
                return None
            return match_one(path, cfg, cfg.root_dir)

        def collect_done(future):
            item = future.result()
            with self._lock:
                if item is None:
                    return
                if item.skipped:
                    summary.skipped += 1
                    self._on_log("warn", "跳过 %s：%s" % (item.rel_path, item.skipped))
                else:
                    summary.scanned += 1
                    if item.hit:
                        summary.hit += 1
                    else:
                        summary.miss += 1
                    items.append(item)
                done = summary.scanned + summary.skipped
                self._progress(done, total, summary.hit, summary.miss, summary.skipped)

        try:
            with ThreadPoolExecutor(max_workers=max(1, cfg.threads)) as ex:
                futures = {ex.submit(worker, p): p for p in files}
                for fut in as_completed(futures):
                    if self._cancel:
                        break
                    try:
                        collect_done(fut)
                    except Exception:
                        with self._lock:
                            summary.skipped += 1
                        self._on_log("err", "处理出错：\n" + traceback.format_exc(limit=3))
                for f in futures:
                    f.cancel()
        except Exception as e:
            self._on_log("err", "扫描异常：%s" % e)

        summary.elapsed = round(time.time() - t0, 2)
        self._progress(total, total, summary.hit, summary.miss, summary.skipped)
        self._on_log("ok", "扫描完成：命中 %d，未命中 %d，跳过 %d，耗时 %.2fs" % (
            summary.hit, summary.miss, summary.skipped, summary.elapsed))
        return items, summary

    @staticmethod
    def _under(path: str, base: str) -> bool:
        try:
            return os.path.commonpath([os.path.abspath(path), base]) == base
        except ValueError:
            return False

    def _progress(self, done, total, hit, miss, skipped):
        d = {"done": done, "total": total, "hit": hit, "miss": miss, "skipped": skipped,
             "pct": (done / total * 100) if total else 0.0}
        self._on_progress(d)
