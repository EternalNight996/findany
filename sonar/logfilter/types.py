# -*- coding: utf-8 -*-
"""日志类型判型（移植 heg-admin-log DataTaskVersion 思路）。

判型优先级（定版 ①A）：
  1. 文件名 AUTO2 前缀                       -> e-autotest
  2. 内容含 OA3 锚点（inject Start / Hash）  -> etest(OA3)
  3. 首个非空行含 ": e-autotest"             -> e-autotest
  4. 内容含尾部结构化 JSON 锚点 "R<{"        -> etest
  5. 其余                                    -> unknown
"""
from __future__ import annotations

import os
from enum import Enum


class LogType(str, Enum):
    AUTO = "auto"          # 仅作 GUI/配置「自动识别」占位，detect 结果不会是它
    ETEST_OA3 = "etest(OA3)"
    ETEST = "etest"
    EAUTOTEST = "e-autotest"
    UNKNOWN = "未知"


ANCHOR_OA3_START = "OA3 inject Start"
ANCHOR_HASH = "<HardwareHash>"
ANCHOR_TAIL_JSON = "R<{"
ANCHOR_AUTOTEST_LINE = ": e-autotest"


def detect_log_type(path: str, text: str) -> LogType:
    """按定版优先级判型。path 仅取文件名前缀，text 为全文文本。"""
    stem = os.path.basename(path)
    if stem.upper().startswith("AUTO2"):
        return LogType.EAUTOTEST
    if ANCHOR_OA3_START in text or ANCHOR_HASH in text:
        return LogType.ETEST_OA3
    for line in text.splitlines():
        s = line.strip()
        if not s:
            continue
        if ANCHOR_AUTOTEST_LINE in s:
            return LogType.EAUTOTEST
        break  # 只看首个非空行
    if ANCHOR_TAIL_JSON in text:
        return LogType.ETEST
    return LogType.UNKNOWN
