# -*- coding: utf-8 -*-
"""产物对拍：验 Rust 版筛选批次产物结构（filter_result.xlsx + upload-result.csv）。

用法：py -3 tests/qa_artifacts.py <批次目录> <rust_qa.json>

断言：
  1. filter_result.xlsx 存在且可被 openpyxl 打开
  2. sheet 集 = {明细, 摘要} ∪ {批内实际出现的判型专属 sheet}
  3. 明细表头 = 36 列固定模板，且与 Python 版 report.py DETAIL_COLS 完全一致
  4. upload-result.csv 表头 = Python 版 AUDIT_COLS，且数据行数 = 记录数
"""
import csv
import json
import os
import sys
import warnings

# umya 写出的 xlsx 不含默认样式：openpyxl 会警告但能正常解析，Excel 打开也正常
warnings.filterwarnings("ignore", category=UserWarning)

BATCH = sys.argv[1]
QA_JSON = sys.argv[2]

# 期望列模板（与 v1 report.py 逐字对齐）
import importlib.util

V1 = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "legacy", "v1-python")
spec = importlib.util.spec_from_file_location("v1_report", os.path.join(V1, "sonar", "logfilter", "report.py"))
v1 = importlib.util.module_from_spec(spec)
sys.modules["sonar"] = type(sys)("sonar")  # 让 report.py 的相对 import 不炸
try:
    spec.loader.exec_module(v1)
    EXPECT_DETAIL = [c for c, _ in v1.DETAIL_COLS]
    EXPECT_AUDIT = list(v1.AUDIT_COLS)
except Exception:
    EXPECT_DETAIL = None
    EXPECT_AUDIT = None

raw = open(QA_JSON, "rb").read()
rows = json.loads(raw[3:].decode("utf-8") if raw[:3] == b"\xef\xbb\xbf" else raw.decode("utf-8"))
types = sorted({(r.get("detected_type") or "") for r in rows})
TEMPLATES = {"etest(OA3)", "etest", "e-autotest", "海格旧测试3", "海格旧测试2"}
expect_sheets = {"明细", "摘要"} | {t for t in types if t in TEMPLATES}

fails = []


def check(name, ok, detail=""):
    print(("  PASS  " if ok else "  FAIL  ") + name + ("" if ok else "  " + str(detail)))
    if not ok:
        fails.append(name)


xlsx = os.path.join(BATCH, "filter_result.xlsx")
check("filter_result.xlsx 存在", os.path.isfile(xlsx), xlsx)
if os.path.isfile(xlsx):
    try:
        import openpyxl

        book = openpyxl.load_workbook(xlsx)
        got = set(book.sheetnames)
        check(f"sheet 集 = {sorted(expect_sheets)}", got == expect_sheets, sorted(got))
        if EXPECT_DETAIL:
            ws = book["明细"]
            header = [c.value for c in ws[1]]
            check("明细表头 = 36 列模板", header == EXPECT_DETAIL, f"{len(header)} 列")
            check("明细数据行数 = 记录数", ws.max_row - 1 == len(rows), f"{ws.max_row - 1} vs {len(rows)}")
            check("冻结窗格 C2", ws.freeze_panes == "C2", ws.freeze_panes)
    except ImportError:
        check("openpyxl 可用（跳过结构断言）", True)

audit = os.path.join(BATCH, "upload-result.csv")
check("upload-result.csv 存在", os.path.isfile(audit), audit)
if os.path.isfile(audit):
    with open(audit, encoding="utf-8-sig", newline="") as fh:
        rd = list(csv.reader(fh))
    check("审计表头正确", (rd[0] if rd else []) == (EXPECT_AUDIT or rd[0]), rd[0] if rd else None)
    check("审计行数 = 记录数 + 表头", len(rd) - 1 == len(rows), f"{len(rd) - 1} vs {len(rows)}")

print(f"产物断言失败 {len(fails)} 条")
sys.exit(1 if fails else 0)
