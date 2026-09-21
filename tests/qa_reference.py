# -*- coding: utf-8 -*-
"""对拍参考：用 Python 版 logfilter 提取同一批样例日志，写 UTF-8 JSON 文件。

用法：py -3 tests/qa_reference.py <日志目录> [输出json]

说明：Python 版 read_text 在 utf-8-sig 严格解码失败时直接抛 UnicodeDecodeError；
Rust 版同链严格解码失败会继续降级并在最后宽容兜底，保证「产线文件绝不因一个字节整批失败」。
对拍时给 Python 侧注入同语义的解码器，保证参照系与 Rust 侧同输入。
"""
import json
import os
import sys

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# 参照系：v1 的 Python 实现在 legacy/v1-python/（HEAD 归档，非运行时依赖）
REF_DIR = os.path.join(ROOT_DIR, "legacy", "v1-python")
sys.path.insert(0, REF_DIR)

from sonar.logfilter import engine as E  # noqa: E402

_ORIG_READ_TEXT = E.read_text  # 先留原函数，避免包装后自递归


def read_text_tolerant(path, max_mb=20.0, encoding="auto"):
    """与 Rust 版一致：先按编码链严格解码，全失败再 utf-8 宽容兜底。"""
    try:
        return _ORIG_READ_TEXT(path, max_mb, encoding)
    except (UnicodeDecodeError, LookupError):
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            return fh.read()


E.read_text = read_text_tolerant

root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT_DIR, "doc", "etest-log")
out_path = sys.argv[2] if len(sys.argv) > 2 else os.path.join(ROOT_DIR, "out", "_py_qa.json")
cfg = E.FilterRunCfg(root_dir=root, extensions=["log"])
eng = E.FilterEngine(cfg)
files = eng._walk()
out = [eng._extract_one(p, root) for p in files]
with open(out_path, "w", encoding="utf-8") as f:
    json.dump(out, f, ensure_ascii=False, indent=1, default=str)
print(f"参考输出 {len(out)} 条 -> {out_path}")
