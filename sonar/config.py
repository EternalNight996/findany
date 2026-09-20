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
    scan_file: str = ""               # 单文件模式（非空时优先于 root_dir）
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

    # ---------- 日志筛选 / 回传（work_mode="filter" 时生效） ----------
    work_mode: str = "scan"           # scan 通用扫描 | filter 日志筛选
    filter_log_type: str = "auto"     # auto | etest(OA3) | etest | e-autotest
    upload_enabled: bool = False      # 数据回传开关
    upload_types: str = "etest(OA3)"  # 自动模式下参与回传的判型（逗号分隔）；选定具体类型时以该类型为准
    upload_dry_run: bool = True       # dry-run：只组包校验，不调 CLI
    upload_cli_path: str = ""         # 空=程序目录(或 doc/devicehashupload)下 intunehelper_cli.exe
    upload_secret_key: str = ""       # SecretKey：config.json 已 gitignore，不进源码/日志
    upload_args: str = "upload --stdin --secret-key ~secret_key~"   # ~key~ 占位符模板
    upload_timeout: float = 60.0      # 单台 CLI 超时（秒）
    upload_retries: int = 3           # 退出码 21 重试次数（1/2/4s 退避）
    upload_stdin: bool = True         # False 时 payload 经 ~payload~ 传参
    filter_countdown: int = 3         # 完成后倒计时（秒），归零自动关
    filter_auto_close: bool = False   # 倒计时归零自动关闭程序
    filter_sn: str = ""               # 方案一：SN 关联日志检索（多文件回传）
    filter_file: str = ""             # 方案二：单文件筛选回传
    auto_start_scan: bool = False     # 启动即自动开始扫描/筛选（勾选持久化）

    def validate(self) -> List[str]:
        errs: List[str] = []
        if self.scan_file:
            if not os.path.isfile(self.scan_file):
                errs.append("扫描文件不存在")
        elif not self.root_dir or not os.path.isdir(self.root_dir):
            errs.append("扫描目录不存在")
        if not (1 <= self.threads <= 64):
            errs.append("线程数需在 1~64 之间")
        if self.work_mode == "scan":
            if not self.keyword.strip():
                errs.append("关键字不能为空")
            if self.mode not in ("inc", "exc"):
                errs.append("匹配模式不合法")
            if self.encoding == "ascii" and len(self.keyword) > 0 and any(ord(c) > 127 for c in self.keyword):
                errs.append("ASCII 编码无法匹配非 ASCII 关键字")
        else:  # filter
            if self.work_mode not in ("scan", "filter"):
                errs.append("工作模式不合法")
            if self.upload_enabled:
                if not self.upload_dry_run and not self.upload_secret_key.strip():
                    errs.append("正式回传需填写 SecretKey（dry-run 可留空）")
                if self.upload_timeout <= 0:
                    errs.append("回传超时需大于 0 秒")
                if self.upload_retries < 0 or self.upload_retries > 10:
                    errs.append("回传重试次数需在 0~10 之间")
            if self.filter_countdown < 3 or self.filter_countdown > 3600:
                errs.append("倒计时需在 3~3600 秒之间")
            if self.filter_sn and not self.root_dir:
                errs.append("SN 关联检索需配置扫描目录")
            if self.filter_file and not os.path.isfile(self.filter_file):
                errs.append("单文件模式：文件不存在")
            if self.filter_sn and self.filter_file:
                errs.append("SN 关联与单文件方案二选一")
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
