# -*- coding: utf-8 -*-
"""配置模型与持久化（findany）."""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field, asdict
from typing import List

APP_NAME = "findany"


@dataclass
class SearchConfig:
    """一次扫描的完整配置。"""
    root_dir: str = ""
    keyword: str = "IT6563"
    mode: str = "inc"                 # 'inc' 包含 | 'exc' 不包含
    threads: int = 8                  # 并发线程 1..64
    extensions: List[str] = field(default_factory=lambda: ["txt", "log", "csv", "md", "xml", "json", "java", "cpp", "py", "ini", "cfg", "html"])
    encoding: str = "auto"            # auto | utf-8 | gbk | gb2312 | utf-16 | latin-1 | ascii
    case_sensitive: bool = False
    recursive: bool = True
    copy_files: bool = False          # 是否将命中文件复制到 out
    record_miss: bool = True          # 是否在 Excel 记录未命中文件
    max_file_mb: float = 20.0         # 超过此大小视为二进制/超大，跳过
    out_dir: str = ""                 # 输出根目录，空=程序目录下的 out

    def validate(self) -> List[str]:
        errs: List[str] = []
        if not self.root_dir or not os.path.isdir(self.root_dir):
            errs.append("扫描目录不存在")
        if not self.keyword.strip():
            errs.append("关键字不能为空")
        if not (1 <= self.threads <= 64):
            errs.append("线程数需在 1~64 之间")
        if self.mode not in ("inc", "exc"):
            errs.append("匹配模式不合法")
        if self.encoding == "ascii" and len(self.keyword) > 0 and any(ord(c) > 127 for c in self.keyword):
            errs.append("ASCII 编码无法匹配非 ASCII 关键字")
        return errs


def default_out_dir(app_dir: str) -> str:
    """输出根目录：程序目录下的 out。"""
    return os.path.join(app_dir, "out")


def _config_path(app_dir: str) -> str:
    return os.path.join(app_dir, "config.json")


def load_config(app_dir: str, cfg: SearchConfig | None = None) -> SearchConfig:
    """从 config.json 加载，覆盖默认。文件不存在时返回默认。"""
    cfg = cfg or SearchConfig()
    path = _config_path(app_dir)
    if os.path.isfile(path):
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            for k, v in data.items():
                if hasattr(cfg, k):
                    setattr(cfg, k, v)
        except Exception:
            pass
    if not cfg.out_dir:
        cfg.out_dir = default_out_dir(app_dir)
    return cfg


def save_config(app_dir: str, cfg: SearchConfig) -> None:
    path = _config_path(app_dir)
    try:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(asdict(cfg), f, ensure_ascii=False, indent=2)
        return
    except Exception:
        pass
