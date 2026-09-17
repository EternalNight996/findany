# -*- coding: utf-8 -*-
"""日志筛选/回传 自校验。

基准：doc/etest-log/oa3-samples.json 与 extract-oa3.ps1 的 9 条断言。
运行：py -3 tests\\test_logfilter.py   （或 pytest tests\\test_logfilter.py）
全部通过输出 ALL PASS，退出码 0；任一失败退出码 1。
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from sonar.logfilter.extractors import extract, read_text  # noqa: E402
from sonar.logfilter.types import HEG3_PREFIXES, LogType, detect_log_type  # noqa: E402
from sonar.logfilter.uploader import (  # noqa: E402
    ST_CONFLICT, ST_DRY_RUN, ST_FAIL, ST_OK,
    UploadProfile, build_payload, missing_fields, render_args,
)

ROOT = Path(__file__).resolve().parents[1]
SAMPLES = ROOT / "doc" / "etest-log"

_results = []


def check(name: str, ok: bool, detail: str = ""):
    _results.append((name, ok, detail))
    print(("  PASS  " if ok else "  FAIL  ") + name + (f"  [{detail}]" if detail and not ok else ""))


def sample_files(station: str):
    d = SAMPLES / station
    return sorted(str(p) for p in d.glob("*.log"))


# ---------- 1) 判型 ----------
def test_detect():
    p15 = str(SAMPLES / "Ift" / "MT71I2GSF-2HG260807250XAG0015.log")
    p25 = str(SAMPLES / "fft" / "MT71I2GSF-2HG260807250XAG0025.log")
    check("detect Ift 0015 = etest(OA3)", detect_log_type(p15, read_text(p15)) == LogType.ETEST_OA3)
    check("detect fft 0025 = e-autotest", detect_log_type(p25, read_text(p25)) == LogType.EAUTOTEST)
    fake = str(SAMPLES / "AUTO2_x.log")
    check("detect AUTO2 prefix = e-autotest", detect_log_type(fake, "anything") == LogType.EAUTOTEST)
    check("detect plain = 未知", detect_log_type("x.log", "no anchors here") == LogType.UNKNOWN)


# ---------- 2) OA3 提取（对照 oa3-samples.json / ps1 断言） ----------
def test_oa3_extraction():
    bench = json.loads((SAMPLES / "oa3-samples.json").read_text(encoding="utf-8-sig"))
    by_file = {b["log_file"]: b for b in bench}

    ift = sample_files("Ift")
    fft = sample_files("fft")
    check("Ift logs = 3", len(ift) == 3)
    check("fft logs = 3", len(fft) == 3)

    pks = set()
    for p in ift + fft:
        text = read_text(p)
        f = extract(p, text)
        name = os.path.basename(p)
        b = by_file[name]
        is_ift = p in ift
        if is_ift:
            check(f"{name[-8:]} every Ift log has OA3", bool(f.get("has_oa3")))
        else:
            check(f"{name[-8:]} no fft log has OA3", not f.get("has_oa3"))
        if not f.get("has_oa3"):
            continue
        check(f"{name[-8:]} OA3 block count = 2 (dedup)", f.get("oa3_block_count") == 2)
        check(f"{name[-8:]} HardwareHash identical in both blocks", f.get("hash_consistent") is True)
        check(f"{name[-8:]} oa3_result = Send station:PASS", f.get("oa3_result") == "Send station:PASS")
        check(f"{name[-8:]} SN equals log file name", f.get("sn") == os.path.splitext(name)[0])
        pks.add(f.get("product_key_id", ""))
        # 与 ps1 采样基准对值
        for key in ("product_key", "product_key_id", "product_key_state",
                    "hardware_hash_len", "hardware_hash_sha256", "hardware_hash_head",
                    "inject_start_at", "json_state", "json_res_value"):
            check(f"{name[-8:]} {key} == 基准", str(f.get(key, "")) == str(b.get(key, "")),
                  f"got={f.get(key)!r} want={b.get(key)!r}")
        # 四字段齐（回传 payload 源）
        payload = build_payload(f, UploadProfile().field_map)
        check(f"{name[-8:]} payload 四字段齐全", not missing_fields(payload))
        check(f"{name[-8:]} baseboard_product = XBoard V7", payload.get("baseboard_product") == "XBoard V7")
        check(f"{name[-8:]} hash len = 4000", f.get("hardware_hash_len") == 4000)
    check("three ProductKeyID are distinct", len(pks) == 3)


# ---------- 2.5) 引擎级 dry-run（覆盖回传循环与类型闸） ----------
def test_engine_dry_run():
    from sonar.logfilter.engine import FilterEngine, FilterRunCfg
    from sonar.logfilter.uploader import UploadProfile
    cfg = FilterRunCfg(
        root_dir=str(SAMPLES), out_dir=str(ROOT / "out"),
        upload_enabled=True, upload_types=["etest(OA3)"], dry_run=True,
        profile=UploadProfile(secret_key="bench"),
    )
    items, s = FilterEngine(cfg).run()
    check("引擎 dry-run 提取 6/6", s.total == 6 and s.extracted == 6)
    check("引擎 dry-run 仅 OA3 参与(3)", s.upload_dry == 3 and s.upload_fail == 0)
    oa3 = [it for it in items if it["detected_type"] == "etest(OA3)"]
    check("引擎 dry-run OA3 状态=dry-run", all(it.get("upload_state") == "dry-run" for it in oa3))
    check("引擎 dry-run 产物落盘", s.excel_path and os.path.isfile(s.excel_path) and os.path.isfile(s.audit_path))


# ---------- 2.7) heg-admin-log 对齐：e-autotest 增强 + 海格旧测试2/3 ----------
def test_heg_alignment():
    # 真样例判型（失败 run，但前缀可判）：BURN/BATTERY -> 海格旧测试3
    hg_dir = ROOT.parent / "hg-autotest" / "hg-autotest-logs"
    if (hg_dir / "BURN.log").is_file():
        check("detect BURN.log = 海格旧测试3",
              detect_log_type(str(hg_dir / "BURN.log"), read_text(str(hg_dir / "BURN.log"))) == LogType.HEG_AUTOTEST3)

    # e-autotest 增强（真样例 0015：app_tag 分发 + MAC 归类）
    p15 = sample_files("Ift")[0]
    f = extract(p15, read_text(p15))
    check("e-autotest 系统SN校验 -> system_sn", f.get("system_sn") == "MT71I2GSF-2HG260807250XAG0015")
    check("e-autotest BIOS版本校验 -> bios_version", f.get("bios_version") == "MT71H-SHP-006-V3.14-F")
    check("e-autotest MAC获取 -> lan(去横杠)", "54014AF088C4" in str(f.get("lan", "")))
    check("e-autotest production_num = 文件名", f.get("production_num") == "MT71I2GSF-2HG260807250XAG0015")
    check("e-autotest oa3_id 别名", f.get("oa3_id") == "4362262499781")

    # 合成海格旧测试3（锚点逐条对齐 heg-admin-log from_heg3）
    heg3 = "\n".join([
        "2026-09-01 [BURN] INFO start",
        "@OS激活码=VK7JB-G8YPH-XXXXX-XXXXX-XXXXX",
        "@OS激活码=烧录失败行应被忽略",
        "@UUID=1234ABCD-5678-90EF-1122-334455667788",
        "@BIOS_SN=SYS_SN_0001",
        "@BOARD_SN=BRD_SN_0002",
        "@BIOS版本=F.15",
        "<ProductKeyID>4362262499781</ProductKeyID>",
        "<ProductKey>QYNK9-GTV9Y-HM8J4-P4M2P-6JH4D</ProductKey>",
        '@网络MAC=[{"interface":"以太网","mac":"AA-BB-CC-DD-EE-01"},'
        '{"interface":"以太网","mac":"00-00-00-00-00-00"},'
        '{"interface":"vEthernet (WSL)","mac":"AA-BB-CC-DD-EE-99"}]',
        "2026-09-01 [BURN] INFO done",
    ])
    f3 = extract("BFT-xxx.log", heg3, LogType.HEG_AUTOTEST3)
    check("heg3 判型=旧测试3", detect_log_type("BFT-xxx.log", heg3) == LogType.HEG_AUTOTEST3)
    check("heg3 OS激活码(忽略烧录)", f3.get("os_key") == "VK7JB-G8YPH-XXXXX-XXXXX-XXXXX")
    check("heg3 UUID", f3.get("uuid") == "1234ABCD-5678-90EF-1122-334455667788")
    check("heg3 BIOS_SN->system_sn", f3.get("system_sn") == "SYS_SN_0001")
    check("heg3 BOARD_SN->board_sn", f3.get("board_sn") == "BRD_SN_0002")
    check("heg3 BIOS版本", f3.get("bios_version") == "F.15")
    check("heg3 oa3_id/oa3_key", f3.get("oa3_id") == "4362262499781" and f3.get("oa3_key") == "QYNK9-GTV9Y-HM8J4-P4M2P-6JH4D")
    check("heg3 有线MAC(去无效/虚拟,保留横杠)", f3.get("lan") == "AA-BB-CC-DD-EE-01")
    check("heg3 production_num", f3.get("production_num") == "BFT-xxx")

    # 合成海格旧测试2（多行接口块：MAC 在接口行后第 3 行）
    heg2 = "\n".join([
        "IFT-START 测试开始",
        "@OS激活码=VK7JB-HEG2-KEY",
        "@UUID=UUID-HEG2-0001",
        "@网络MAC=以下接口:",
        "以太网 接口:",
        "   连接状态: 已连接",
        "   速率: 1000",
        "   MAC地址: 11-22-33-44-55-66",
        "WLAN 接口:",
        "   连接状态: 已断开",
        "   速率: 0",
        "   MAC地址: 77-88-99-AA-BB-CC",
    ])
    f2 = extract("IFT-START-xxx.log", heg2, LogType.HEG_AUTOTEST2)
    check("heg2 判型=旧测试2(先于IFT前缀)", detect_log_type("IFT-START-xxx.log", heg2) == LogType.HEG_AUTOTEST2)
    check("heg2 OS激活码", f2.get("os_key") == "VK7JB-HEG2-KEY")
    check("heg2 UUID", f2.get("uuid") == "UUID-HEG2-0001")
    check("heg2 有线MAC(j+3)", f2.get("lan") == "11-22-33-44-55-66")
    check("heg2 无线MAC(j+3)", f2.get("wifilan") == "77-88-99-AA-BB-CC")
    check("heg2 production_num", f2.get("production_num") == "IFT-START-xxx")

    # 前缀表回归：IFT-START 优先于 IFT
    check("IFT-START 前缀优先于 IFT",
          detect_log_type("IFT-START-x.log", "no content") == LogType.HEG_AUTOTEST2
          and detect_log_type("IFT-x.log", "no content") == LogType.HEG_AUTOTEST3)


# ---------- 3) dry-run 与判定/重试（注入桩，不碰网） ----------
def _fields_of_first():
    p = sample_files("Ift")[0]
    return extract(p, read_text(p))


def test_dry_run_and_judge():
    prof = UploadProfile(cli_path=r"C:\fake\cli.exe", secret_key="sk-test", retry_delay=0)
    fields = _fields_of_first()

    res = __import__("sonar.logfilter.uploader", fromlist=["run_upload"]).run_upload(prof, fields, dry_run=True)
    check("dry-run 状态 = dry_run", res.status == ST_DRY_RUN)
    payload = json.loads(res.payload_json)
    check("dry-run payload serial_number 正确", payload["serial_number"] == fields["sn"])
    check("dry-run payload hash 长度 4000", len(payload["hardware_hash"]) == 4000)

    def runner_ok(cmd, stdin_b, use_stdin, timeout):
        return 0, '{"status":"accepted","request_id":"rid-1","serial_number":"x"}', ""

    def runner_dup(cmd, stdin_b, use_stdin, timeout):
        return 0, '{"status":"duplicate_accepted","request_id":"rid-2"}', ""

    def runner_conflict(cmd, stdin_b, use_stdin, timeout):
        return 12, '{"status":"conflict","request_id":"rid-3"}', ""

    def runner_net_then_ok(cmd, stdin_b, use_stdin, timeout):
        runner_net_then_ok.n = getattr(runner_net_then_ok, "n", 0) + 1
        if runner_net_then_ok.n == 1:
            return 21, "", "net"
        return 0, '{"status":"accepted","request_id":"rid-4"}', ""

    def runner_bad(cmd, stdin_b, use_stdin, timeout):
        return 10, "", "bad json"

    from sonar.logfilter.uploader import run_upload
    check("判定 0+accepted = ok", run_upload(prof, fields, runner=runner_ok).status == ST_OK)
    check("判定 0+duplicate_accepted = ok", run_upload(prof, fields, runner=runner_dup).status == ST_OK)
    r = run_upload(prof, fields, runner=runner_conflict)
    check("判定 12 = conflict", r.status == ST_CONFLICT and r.request_id == "rid-3")
    r = run_upload(prof, fields, runner=runner_net_then_ok)
    check("判定 21 重试后 0 = ok(2次)", r.status == ST_OK and r.attempts == 2)
    r = run_upload(prof, fields, runner=runner_bad)
    check("判定 10 = fail 不重试", r.status == ST_FAIL and r.attempts == 1)
    r = run_upload(prof, {"sn": "x"}, runner=runner_ok)
    check("字段不全 = fail 且不调 CLI", r.status == ST_FAIL and "字段不全" in r.error)

    args = render_args("upload --stdin --secret-key ~secret_key~", prof, "{}", fields)
    check("参数模板渲染 secret_key", args == ["upload", "--stdin", "--secret-key", "sk-test"])


def main() -> int:
    test_detect()
    test_oa3_extraction()
    test_heg_alignment()
    test_engine_dry_run()
    test_dry_run_and_judge()
    fails = [n for n, ok, _ in _results if not ok]
    print()
    print(f"checks: {len(_results)}  failed: {len(fails)}")
    if fails:
        print("FAILED:", "; ".join(fails))
        return 1
    print("ALL PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
