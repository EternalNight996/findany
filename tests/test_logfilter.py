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
import tomllib
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


# ---------- 2.8) TOML 自动化（方案一 SN 关联多文件 / 方案二 单文件） ----------
def test_auto_toml():
    from sonar.logfilter import autoconfig as ac
    from sonar.logfilter.engine import FilterEngine, FilterRunCfg
    from sonar.logfilter.uploader import UploadProfile

    toml = """
[filter]
root_dir = "D:/logs"
log_type = "auto"
recursive = true
[run]
auto_start = true
countdown_sec = 3
auto_close = true
[upload]
enabled = true
dry_run = true
types = ["etest(OA3)"]
secret_key = "sk-toml"
[scheme]
mode = "sn_dir"
sn = "MT71I2GSF"
"""
    auto = ac.parse_auto(tomllib.loads(toml))
    check("toml 解析 enabled/倒计时3s", auto.enabled and auto.countdown_sec == 3)
    check("toml 方案/dry-run/密钥", auto.scheme == "sn_dir" and auto.sn == "MT71I2GSF"
          and auto.dry_run is True and auto.secret_key == "sk-toml")

    # 方案一：SN 关联检索（文件名+内容双通道；扩展名默认 log）
    hits = ac.find_sn_logs(str(SAMPLES), "MT71I2GSF-2HG260807250XAG0015", True, ["log"])
    check("SN 精确检索=1 份", len(hits) == 1 and hits[0].endswith("0015.log"))
    hits6 = ac.find_sn_logs(str(SAMPLES), "MT71I2GSF", True, ["log"])
    check("SN 前缀检索=6 份", len(hits6) == 6)
    hits_pk = ac.find_sn_logs(str(SAMPLES), "4362262499781", True, ["log"])   # 仅内容含 PKID
    check("SN 内容命中(文件名不含)", len(hits_pk) == 1 and hits_pk[0].endswith("0015.log"))

    # 方案一 引擎：file_list 通道（dry-run 全链）
    cfg = FilterRunCfg(root_dir=str(SAMPLES), out_dir=str(ROOT / "out"),
                       file_list=hits, upload_enabled=True, dry_run=True,
                       profile=UploadProfile(secret_key="x"))
    items, s = FilterEngine(cfg).run()
    check("方案一 引擎: total=1 dry=1", s.total == 1 and s.upload_dry == 1)

    # 方案二 引擎：单文件
    cfg2 = FilterRunCfg(root_dir=str(SAMPLES), out_dir=str(ROOT / "out"),
                        file_list=[sample_files("Ift")[1]], upload_enabled=True, dry_run=True,
                        profile=UploadProfile(secret_key="x"))
    items2, s2 = FilterEngine(cfg2).run()
    check("方案二 引擎: total=1 提取成功", s2.total == 1 and s2.extracted == 1)

    # toml -> SearchConfig 覆盖
    from sonar.config import SearchConfig
    cfgS = SearchConfig()
    ac.apply_to_config(auto, cfgS)
    check("toml 覆盖 SearchConfig", cfgS.work_mode == "filter" and cfgS.filter_sn == "MT71I2GSF"
          and cfgS.upload_dry_run is True and cfgS.filter_countdown == 3)
    check("覆盖后校验过(SN 有目录)", cfgS.validate() == [] or all("SN" not in e for e in cfgS.validate()))


# ---------- 2.9) toml 缺席时生成默认模板 ----------
def test_default_toml_generation():
    import tempfile
    from sonar.logfilter import autoconfig as ac

    with tempfile.TemporaryDirectory() as td:
        ns = ac.parse_args([])                       # 无 --config：走默认路径
        app_dir = td
        auto = ac.resolve_auto(ns, app_dir)
        default_path = os.path.join(app_dir, "findany.toml")
        check("默认路径不存在 → 生成模板", auto.generated is True and os.path.isfile(default_path))
        check("模板不自动开跑", auto.enabled is False)
        # 模板本身可被 tomllib 解析且字段有效
        parsed = ac.parse_auto(ac.load_toml(default_path))
        check("模板可解析且 dry_run=true", parsed.dry_run is True and parsed.auto_start is False
              and parsed.countdown_sec == 3 and parsed.scheme == "sn_dir")

        # 二次启动：模板已存在 → 不覆盖、不再标记 generated
        auto2 = ac.resolve_auto(ac.parse_args([]), app_dir)
        check("已存在不覆盖", auto2.generated is False and auto2.enabled is False)

        # 显式 --config 指向缺失路径 → 也生成模板
        custom = os.path.join(td, "sub", "my.toml")
        auto3 = ac.resolve_auto(ac.parse_args(["--config", custom]), td)
        check("显式缺失路径也生成", auto3.generated is True and os.path.isfile(custom))

        # 已有配置不受影响：写入后 resolve 读用户值
        with open(default_path, "w", encoding="utf-8") as f:
            f.write("[run]\nauto_start = true\ncountdown_sec = 5\n[filter]\nroot_dir = \"" + str(SAMPLES).replace("\\", "/") + "\"\n[scheme]\nmode = \"sn_dir\"\nsn = \"MT71I2GSF\"\n")
        auto4 = ac.resolve_auto(ac.parse_args([]), app_dir)
        check("用户配置正常读取且 enabled", auto4.enabled is True and auto4.sn == "MT71I2GSF"
              and auto4.countdown_sec == 5)


# ---------- 2.9) 共享项生效：编码链 + SN 检索按扩展名过滤 ----------
def test_shared_options():
    import tempfile
    from sonar.logfilter import autoconfig as ac
    from sonar.logfilter.extractors import read_text

    # 编码共享项：指定 gbk 链可解 GBK 字节；auto 链也能解
    with tempfile.NamedTemporaryFile(suffix=".log", delete=False) as fh:
        fh.write("系统SN=MT71I2GSF-GBK测试\nOA3=Send station:PASS".encode("gbk"))
        p_gbk = fh.name
    check("read_text 编码=gbk 生效", "MT71I2GSF-GBK测试" in read_text(p_gbk, encoding="gbk"))
    check("read_text 编码=auto 兜底", "Send station:PASS" in read_text(p_gbk))
    os.unlink(p_gbk)

    # SN 检索按扩展名白名单过滤（共享「文件扩展名」）
    hits_log = ac.find_sn_logs(str(SAMPLES), "MT71I2GSF", True, ["log"])
    check("SN 检索 ext=log", len(hits_log) == 6 and all(h.endswith(".log") for h in hits_log))
    hits_csv = ac.find_sn_logs(str(SAMPLES), "4362262499781", True, ["csv"])
    check("SN 检索 ext=csv（内容命中采样文件）", len(hits_csv) >= 1 and hits_csv[0].endswith(".csv"))
    hits_none = ac.find_sn_logs(str(SAMPLES), "MT71I2GSF", True, ["xlsx"])
    check("SN 检索 ext 不匹配=0", len(hits_none) == 0)


# ---------- 2.10) 统一 Excel 模板：分组列序 + 空列隐藏 + 宽度/冻结 ----------
def test_excel_template():
    from sonar.logfilter import report
    from sonar.logfilter.report import export_filter_excel
    import tempfile
    openpyxl_ok = report._try_openpyxl() is not None
    if not openpyxl_ok:
        check("openpyxl 缺失跳过模板断言", True)
        return

    # 键集合不变（仅重排）：与旧 35 键逐一对应
    old_keys = {"idx", "log_file", "dir_name", "rel_path", "station", "detected_type", "sn",
                "oa3_result", "product_key_id", "product_key_state", "hardware_hash_len",
                "hardware_hash_sha256", "hardware_hash", "inject_start_at", "inject_end_at", "product_key",
                "baseboard_product", "mo_lot_no", "task_tag", "json_state", "json_res_value",
                "project_version", "production_num", "system_sn", "board_sn", "uuid",
                "bios_version", "os_key", "lan", "wifilan", "bluetooth", "extract_state",
                "upload_state", "upload_code", "request_id", "upload_error"}
    check("模板 36 键集合(增HardwareHash)", {k for _, k in report.DETAIL_COLS} == old_keys)
    check("分组顺序: 设备紧随识别", [k for _, k in report.DETAIL_COLS][:12]
          == ["idx", "log_file", "dir_name", "rel_path", "station", "detected_type",
              "sn", "production_num", "system_sn", "board_sn", "uuid", "bios_version"])

    rows = [{"log_file": "0015.log", "sn": "SN0015", "detected_type": "etest(OA3)",
             "extract_state": "成功", "upload_state": "dry_run",
             "hardware_hash_sha256": "A" * 64, "hardware_hash": "H" * 4000},
            {"log_file": "BURN.log", "sn": "SNBURN", "detected_type": "海格旧测试3",
             "extract_state": "成功", "lan": "AA-BB-CC-DD-EE-01"}]
    with tempfile.TemporaryDirectory() as td:
        out = os.path.join(td, "tpl.xlsx")
        export_filter_excel(out, rows, [("总数", 2)])
        import openpyxl
        book = openpyxl.load_workbook(out)
        ws = book["明细"]
        headers = [c.value for c in ws[1]]
        assert headers == [h for h, _ in report.DETAIL_COLS]   # 列序与模板一致
        col = {k: i + 1 for i, (_, k) in enumerate(report.DETAIL_COLS)}   # 字段键 → 列号
        from openpyxl.utils import get_column_letter as gcl
        hidden = lambda k: ws.column_dimensions[gcl(col[k])].hidden
        check("全空列隐藏(蓝牙MAC/PKID/回传错误)", hidden("bluetooth") and hidden("product_key_id")
              and hidden("upload_error"))
        check("非空列可见(SN/判型/有线MAC)", not hidden("sn") and not hidden("detected_type")
              and not hidden("lan"))
        check("冻结 C2(表头+序号/文件)", ws.freeze_panes == "C2")
        check("SHA-256 列宽固定偏好", ws.column_dimensions[gcl(col["hardware_hash_sha256"])].width == 20)
        check("自适应宽度不超上限", ws.column_dimensions[gcl(col["sn"])].width <= 40)
        check("HardwareHash 本体列可见且全值在格", not hidden("hardware_hash")
              and ws.cell(row=2, column=col["hardware_hash"]).value == "H" * 4000)
        check("OA3 专属sheet含HardwareHash键", "hardware_hash" in report.TYPE_TEMPLATES["etest(OA3)"][0:1]
              or "hardware_hash" in [k for _, k in report.TYPE_TEMPLATES["etest(OA3)"]])

        # 类型专属 sheet：按判型动态生成
        check("类型sheet按需生成", book.sheetnames == ["明细", "etest(OA3)", "海格旧测试3", "摘要"])
        ws_oa3 = book["etest(OA3)"]
        oa3_keys = [k for _, k in report.TYPE_TEMPLATES["etest(OA3)"]]
        check("OA3 sheet 列序与模板一致", [c.value for c in ws_oa3[1]] == [h for h, _ in report.TYPE_TEMPLATES["etest(OA3)"]])
        check("OA3 sheet 行数=1(仅OA3文件)", ws_oa3.max_row == 2)
        check("OA3 sheet 含Hash列", "hardware_hash_sha256" in oa3_keys)
        ws_hg = book["海格旧测试3"]
        hg_keys = [k for _, k in report.TYPE_TEMPLATES["海格旧测试3"]]
        check("海格sheet 含设备+PKID核", "board_sn" in hg_keys and "product_key_id" in hg_keys
              and "hardware_hash_len" not in hg_keys)
        check("海格sheet 行数=1", ws_hg.max_row == 2)
        # 未知类型不生成 sheet，只在总表兜底
        rows_u = rows + [{"log_file": "x.log", "detected_type": "未知", "extract_state": "失败"}]
        out2 = os.path.join(td, "tpl2.xlsx")
        export_filter_excel(out2, rows_u, [("总数", 3)])
        book2 = openpyxl.load_workbook(out2)
        check("未知类型不生成专属sheet", book2.sheetnames == ["明细", "etest(OA3)", "海格旧测试3", "摘要"])
        check("未知行落在总表", book2["明细"].max_row == 4)



# ---------- 2.11) 保存配置 → toml 同步（合并写，注释/auto_start 保留） ----------
def test_toml_sync():
    import tempfile
    from sonar.logfilter import autoconfig as ac
    from sonar.config import SearchConfig

    toml = """# 手工注释头
[filter]
root_dir = "D:\\old"          # 扫描目录注释
[run]
auto_start = true              # 自动化开关不动
countdown_sec = 30
[upload]
dry_run = true
[scheme]
mode = "sn_dir"
sn = "OLD-SN"
"""
    td = tempfile.mkdtemp(); p = os.path.join(td, "findany.toml")
    open(p, "w", encoding="utf-8").write(toml)
    cfg = SearchConfig()
    cfg.root_dir = "F:\\logs\\new"
    cfg.filter_countdown = 3
    cfg.filter_auto_close = False
    cfg.upload_dry_run = False
    cfg.upload_secret_key = "sk-abc"
    cfg.filter_sn = "MT71-0017"
    ac.sync_toml(p, cfg)
    raw = open(p, encoding="utf-8").read()
    d = tomllib.loads(raw)
    check("同步: 注释保留", "# 手工注释头" in raw and "# 扫描目录注释" in raw)
    check("同步: auto_start 未动", d["run"]["auto_start"] is True)
    check("同步: 路径字面量串更新", d["filter"]["root_dir"] == "F:\\logs\\new")
    check("同步: 倒计时/关窗", d["run"]["countdown_sec"] == 3 and d["run"]["auto_close"] is False)
    check("同步: dry_run/secret_key 追加", d["upload"]["dry_run"] is False
          and d["upload"].get("secret_key") == "sk-abc")
    check("同步: SN 更新", d["scheme"]["sn"] == "MT71-0017")
    p2 = os.path.join(td, "sub", "new.toml")
    ac.sync_toml(p2, cfg)
    d2 = tomllib.loads(open(p2, encoding="utf-8").read())
    check("缺失 toml 生成并同步", d2["filter"]["root_dir"] == "F:\\logs\\new")
    cfg2 = SearchConfig(); ac.apply_to_config(ac.parse_auto(ac.load_toml(p)), cfg2)
    check("同步后 GUI 侧可还原", cfg2.root_dir == "F:\\logs\\new" and cfg2.filter_sn == "MT71-0017"
          and cfg2.upload_dry_run is False)


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
    test_auto_toml()
    test_default_toml_generation()
    test_shared_options()
    test_excel_template()
    test_toml_sync()
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
