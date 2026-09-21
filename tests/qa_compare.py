# -*- coding: utf-8 -*-
"""Rust 版 vs Python 版 字段对拍。

比对范围为「Python 提取器实际产出的字段」（含嵌套 fields）：
超集不算差异（Rust 版额外落库的字段单独列出），两侧都有的字段必须逐字相同。
"""
import json
import sys


def load(path):
    raw = open(path, "rb").read()
    if raw[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return json.loads(raw.decode("utf-16"))
    if raw[:3] == b"\xef\xbb\xbf":
        return json.loads(raw[3:].decode("utf-8"))
    return json.loads(raw.decode("utf-8"))


def norm(v):
    if v is None:
        return ""
    if isinstance(v, bool):
        return "True" if v else "False"
    return str(v)


def key_of(row):
    name = row.get("log_file") or row.get("filename") or row.get("rel_path")
    return norm(name)


rust = load(sys.argv[1])
py = load(sys.argv[2])

by_rust = {}
by_py = {}
for r in rust:
    by_rust[key_of(r)] = r
for r in py:
    by_py[key_of(r)] = r

names = sorted(set(by_rust) | set(by_py))
diffs = []
extra = []
checked = 0
fields_checked = 0
for n in names:
    a, b = by_rust.get(n), by_py.get(n)
    if a is None or b is None:
        diffs.append((n, "(文件存在性)", "有" if a else "无", "有" if b else "无"))
        continue
    for k, vb in b.items():
        if k == "fields":
            continue
        va = norm(a.get(k))
        vb2 = norm(vb)
        fields_checked += 1
        if va != vb2:
            diffs.append((n, k, va[:140], vb2[:140]))
    fa = a.get("fields") or {}
    for k, vb in (b.get("fields") or {}).items():
        va = norm(fa.get(k))
        vb2 = norm(vb)
        fields_checked += 1
        if va != vb2:
            diffs.append((n, "fields." + k, va[:140], vb2[:140]))
    for k in a:
        if k == "fields" or k not in b:
            continue
        extra.append((n, k))
    checked += 1

print(f"文件 {checked}/{len(names)} 对拍；Python 侧字段断言 {fields_checked} 条；差异 {len(diffs)} 条")
for d in diffs[:60]:
    print(f"  [{d[0]}] {d[1]}:\n      rust = {d[2]!r}\n      py   = {d[3]!r}")
uniq_extra = sorted({k for _, k in extra})
if uniq_extra:
    print(f"Rust 侧额外字段（超集，非差异）{len(uniq_extra)} 个: {', '.join(uniq_extra[:40])}")
sys.exit(1 if diffs else 0)
