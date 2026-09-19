# -*- coding: utf-8 -*-
"""TOML 自动化配置：findany.toml 驱动「检测→回传→倒计时关」流程。

用法（模拟自动化，无需手动点开始）：
  findany.exe --config findany.toml [--sn 设备SN] [--file 单文件]
  或直接把 findany.toml 放程序目录且 run.auto_start=true

回传方案（scheme.mode）：
  sn_dir  —— 方案一：动态 SN 检索关联日志（文件名或内容命中，多文件回传）
  single  —— 方案二：指定单文件筛选回传

findany.toml 示例：
  [filter]
  root_dir = "D:\\logs"
  log_type = "auto"            # auto|etest(OA3)|etest|e-autotest|海格旧测试2|海格旧测试3
  recursive = true

  [run]
  auto_start = true
  countdown_sec = 3            # 完成后倒计时，归零自动关
  auto_close = true

  [upload]
  enabled = true
  dry_run = false
  types = ["etest(OA3)"]
  cli_path = ""
  secret_key = ""
  args = "upload --stdin --secret-key ~secret_key~"
  timeout_sec = 60
  max_retries = 3

  [scheme]
  mode = "sn_dir"              # sn_dir | single
  sn = ""                      # 方案一：设备 SN（可被 --sn 覆盖）
  file = ""                    # 方案二：单文件路径（可被 --file 覆盖）
"""
from __future__ import annotations

import argparse
import os
import tomllib
from dataclasses import dataclass, field
from typing import List, Optional

SN_READ_CAP = 64 * 1024 * 1024   # 单文件 SN 内容检索上限（防超大文件拖死）


@dataclass
class AutoRun:
    """一次自动化运行的完整意图。"""
    enabled: bool = False
    auto_start: bool = False
    root_dir: str = ""
    log_type: str = "auto"
    recursive: bool = True
    extensions: List[str] = field(default_factory=lambda: ["log"])
    scheme: str = "sn_dir"        # sn_dir | single
    sn: str = ""
    file: str = ""
    upload_enabled: bool = True
    upload_types: List[str] = field(default_factory=lambda: ["etest(OA3)"])
    dry_run: bool = False
    cli_path: str = ""
    secret_key: str = ""
    args: str = "upload --stdin --secret-key ~secret_key~"
    timeout_sec: float = 60.0
    max_retries: int = 3
    countdown_sec: int = 3
    auto_close: bool = True
    generated: bool = False        # 本次为自动生成默认模板（非用户已有配置）


def parse_args(argv: Optional[List[str]] = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(prog="findany", description="findany 日志筛选/回传（TOML 自动化）")
    p.add_argument("--config", default="", help="TOML 配置路径（缺省=程序目录 findany.toml）")
    p.add_argument("--sn", default="", help="方案一：设备 SN，覆盖 toml（隐含 sn_dir）")
    p.add_argument("--file", default="", help="方案二：单文件路径，覆盖 toml（隐含 single）")
    return p.parse_args(argv)


def load_toml(path: str) -> dict:
    with open(path, "rb") as f:
        return tomllib.load(f)


DEFAULT_TOML = """# findany 自动化配置（本文件由程序自动生成；改 auto_start = true 即自动开跑）

[filter]
root_dir = ""                  # 方案一：SN 检索根目录；必填
log_type = "auto"              # auto | etest(OA3) | etest | e-autotest | 海格旧测试2 | 海格旧测试3
recursive = true

[run]
auto_start = false             # 改 true：启动即自动「检测→回传→倒计时关」
countdown_sec = 3              # 完成后倒计时，归零自动关程序
auto_close = true

[upload]
enabled = true
dry_run = true                 # 先 true 演练（只组包不打网），无误后改 false
types = ["etest(OA3)"]         # 参与回传的判型
cli_path = ""                  # 空=程序目录下 intunehelper_cli.exe
secret_key = ""                # 正式回传必填；本文件勿提交仓库
args = "upload --stdin --secret-key ~secret_key~"
timeout_sec = 60
max_retries = 3

[scheme]
mode = "sn_dir"                # sn_dir=方案一(SN关联多文件) | single=方案二(单文件)
sn = ""                        # 方案一：设备 SN（--sn 可覆盖）
file = ""                      # 方案二：单文件路径（--file 可覆盖）
"""


def _write_default(path: str) -> bool:
    """输出默认 toml；成功 True。已存在时不覆盖。"""
    if os.path.exists(path):
        return False
    try:
        os.makedirs(os.path.dirname(os.path.abspath(path)) or ".", exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(DEFAULT_TOML)
        return True
    except OSError:
        return False


def resolve_auto(ns: argparse.Namespace, app_dir: str) -> AutoRun:
    """toml 不存在时输出一份默认模板，返回 enabled=False（GUI 模式，模板待编辑）。
    显式 --config 缺失同样生成模板而非报错。CLI --sn/--file 覆盖并隐含方案。"""
    path = ns.config or os.path.join(app_dir, "findany.toml")
    generated = False
    if not os.path.isfile(path):
        return AutoRun(enabled=False, generated=_write_default(path))
    auto = parse_auto(load_toml(path))
    if ns.sn:
        auto.sn, auto.scheme, auto.enabled = ns.sn, "sn_dir", True
    if ns.file:
        auto.file, auto.scheme, auto.enabled = ns.file, "single", True
    return auto


def parse_auto(data: dict) -> AutoRun:
    f = data.get("filter") or {}
    r = data.get("run") or {}
    u = data.get("upload") or {}
    s = data.get("scheme") or {}
    return AutoRun(
        enabled=bool(r.get("auto_start", False)),
        auto_start=bool(r.get("auto_start", False)),
        root_dir=str(f.get("root_dir", "") or ""),
        log_type=str(f.get("log_type", "auto") or "auto"),
        recursive=bool(f.get("recursive", True)),
        extensions=[str(x) for x in (f.get("extensions") or ["log"])],
        scheme=str(s.get("mode", "sn_dir") or "sn_dir"),
        sn=str(s.get("sn", "") or ""),
        file=str(s.get("file", "") or ""),
        upload_enabled=bool(u.get("enabled", True)),
        upload_types=[str(t) for t in (u.get("types") or ["etest(OA3)"])],
        dry_run=bool(u.get("dry_run", False)),
        cli_path=str(u.get("cli_path", "") or ""),
        secret_key=str(u.get("secret_key", "") or ""),
        args=str(u.get("args", "upload --stdin --secret-key ~secret_key~") or ""),
        timeout_sec=float(u.get("timeout_sec", 60)),
        max_retries=int(u.get("max_retries", 3)),
        countdown_sec=int(r.get("countdown_sec", 3)),
        auto_close=bool(r.get("auto_close", True)),
    )


def apply_to_config(auto: AutoRun, cfg) -> None:
    """把 AutoRun 覆盖到 SearchConfig（GUI 侧收集后再走统一校验/启动）。"""
    cfg.work_mode = "filter"
    cfg.root_dir = auto.root_dir or cfg.root_dir
    cfg.filter_log_type = auto.log_type or cfg.filter_log_type
    cfg.recursive = auto.recursive
    cfg.extensions = auto.extensions or cfg.extensions
    cfg.upload_enabled = auto.upload_enabled
    cfg.upload_types = ",".join(auto.upload_types) if auto.upload_types else cfg.upload_types
    cfg.upload_dry_run = auto.dry_run
    cfg.upload_cli_path = auto.cli_path or cfg.upload_cli_path
    cfg.upload_secret_key = auto.secret_key or cfg.upload_secret_key
    cfg.upload_args = auto.args or cfg.upload_args
    cfg.upload_timeout = auto.timeout_sec
    cfg.upload_retries = auto.max_retries
    cfg.filter_countdown = auto.countdown_sec
    cfg.filter_auto_close = auto.auto_close
    if auto.scheme == "single":
        cfg.filter_file, cfg.filter_sn = auto.file, ""
    else:
        cfg.filter_sn, cfg.filter_file = auto.sn, ""


def find_sn_logs(root: str, sn: str, recursive: bool = True, extensions: Optional[List[str]] = None) -> List[str]:
    """方案一：SN 关联日志检索。文件名或内容命中即纳入（多文件）。
    extensions：扩展名白名单（共享项，如 ["log"]）；None/空 = 不限。"""
    out: List[str] = []
    if not sn or not os.path.isdir(root):
        return out
    exts = {e.lower().lstrip(".") for e in extensions} if extensions else None
    needle = sn.encode("utf-8", errors="ignore")
    for cur, dirs, files in os.walk(root):
        if not recursive:
            dirs[:] = []
        for f in files:
            if f.startswith("."):
                continue
            if exts is not None and f.rsplit(".", 1)[-1].lower() not in exts:
                continue
            p = os.path.join(cur, f)
            if sn in f or sn.upper() in f.upper():
                out.append(p)
                continue
            try:
                if os.path.getsize(p) > SN_READ_CAP:
                    continue
                with open(p, "rb") as fh:
                    if needle in fh.read():
                        out.append(p)
            except OSError:
                continue
    return sorted(out)
