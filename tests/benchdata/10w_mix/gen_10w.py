"""10W+ 黄金数据生成器：1万真 OA3 + 9万假（不含 OA3 锚点）。

主上用途：自己压测 findany v2.0.0 的 Bug-2A（强制 OA3 真伪校验）和 LRU 缓存。
真样本 = 真含 OA3 双锚点（OA3 inject Start + <HardwareHash>）且 hash 4000 字符 →
           detect 真值 = etest(OA3)；强制 etest(OA3) 提取也走 OA3 路径。
假样本 = e-autotest 风格日志，**不含** OA3 锚点（不含 OA3 inject Start、<HardwareHash>） →
           detect 真值 = e-autotest；强制 etest(OA3) 提取必须降级（Bug-2A 验证点）。

输出结构：
    tests/benchdata/10w_mix/
    ├── manifest.json            每文件预期 detected_type + rel_path（用于自动校验）
    ├── real/                    10000 个真 OA3 日志
    │   ├── batch_00/MT71I2GSF-...-0001.log ...
    │   ...
    │   └── batch_09/...
    └── fake/                    90000 个 e-autotest 风格日志
        ├── batch_00/AUTO2_bench_00001.log ...
        ...
        └── batch_89/...

运行：py -3 tests/benchdata/10w_mix/gen_10w.py
选项：
    --root=DIR                  自定义输出根（默认本脚本同目录）
    --real=10000                真样本数
    --fake=90000                假样本数
    --per-dir=1000              每子目录文件数
    --seed=0                    随机种子（默认 0 = 固定输出可复现）

实现说明：
    · 真样本结构对齐 doc/etest-log/Ift 真实样本（时间戳 + INFO/WARN/ERROR 级别 + tag + msg）
    · 真样本 HardwareHash 用 ASCII 4000 字符（用 base64 编码 3000 字节随机）→ 验证 hash_len=4000
    · 假样本 1KB~50KB 随机填充（用 INFO 级别假 tag + 假 msg）
    · 真假样本**都**用"分批子目录"切分（每 1000 文件一个子目录）—— NTFS 上单目录百万文件
      性能差；findany 也走 batch_dirs 跑起来不会卡 walk_files_filtered。
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import random
import sys
import time
from pathlib import Path

# 真 OA3 样本的"组件"：日期头 / INFO / WARN / ERROR / R<{...}>R 收尾
# 全部时间戳用同一 base 日期 + 随机偏移，让真样本看起来像同一批次跑的
BASE_DATE = "2026-09-22"
TIME_RANGE = 86400  # 一整天的秒数，用来随机散开

# 真 OA3 hash base64 长度：4000 字符 → 解码后 ~3000 字节
HASH_BYTES = 3000

# e-autotest 风格的假 tag
FAKE_TAGS = [
    "信息", "下载配置", "登录", "自动测试", "跟踪", "自动测试结果",
    "刷新", "FTP", "硬件检测", "主板检测", "网管", "系统激活",
    "MAC获取", "BIOS版本校验", "UUID校验", "系统SN校验", "板卡SN校验",
]
FAKE_MSG_POOL = [
    '校验通过',
    '正在运行等待中...',
    'FAIL: 检测失败',
    'PASS: 12',
    '刷新 1 次',
    '当前 MAC: 54-01-4A-F0-88-C4',
    '当前 BIOS 版本: MT71H-SHP-006-V3.14-F',
    '当前 UUID: 12345678-ABCD-1234-EF00-000000000001',
    '系统 SN: MT71I2GSF-2HG260807250XAG0001',
    '主板 SN: BRD-SN-00000001',
    '连接状态: 已连接',
    '速率: 1000Mbps',
    'IP MAC: (11.11.11.86 54-01-4A-F0-88-C4)',
    '语言环境: zh-CN,en-US',
    '时区: (UTC+08:00) 北京',
]

# 假样本尾部 R<{...}>R 摘要（让 e-autotest 路径的 data_items 也吃得到东西）
FAKE_TAIL_JSON = '{"content":"PASS","status":true,"opts":{"api":"AutoTest","task":"full_check","init":false,"full":false,"filter":[],"args":[],"command":[],"data":[]}}'


def gen_timestamp(rng: random.Random, offset_seconds: int) -> str:
    """返回 'YYYY-MM-DD HH:MM:SS.mmm' 格式时间戳。offset_seconds 是相对 BASE_DATE 的偏移。"""
    t = time.struct_time((2026, 9, 22, 0, 0, 0, 0, 0, 0))
    base_ts = int(time.mktime(t))
    full_ts = base_ts + offset_seconds
    ms = rng.randint(0, 999)
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(full_ts)) + f".{ms:03d}"


def gen_hash(rng: random.Random) -> str:
    """生成 4000 字符的 base64 串（与 etest(OA3) 真实硬件 hash 同形）。"""
    raw = bytes(rng.getrandbits(8) for _ in range(HASH_BYTES))
    return base64.b64encode(raw).decode("ascii")[:4000]


def gen_real_oa3(rng: random.Random, idx: int) -> str:
    """生成一份真 OA3 日志。"""
    sn = f"MT71I2GSF-2HG260807250XAG{idx:04d}"
    pkid = 4362262499781 + (idx % 100)  # PKID 在 1万份里稳定有规律，便于人工核对
    pk_state = "3"
    hardware_hash = gen_hash(rng)
    base_offset = rng.randint(0, TIME_RANGE - 60)

    lines: list[str] = []
    # 头部：项目 + 登录 + 几个假 INFO 行（伪装 e-autotest 风格）
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"信息\" msg=\"项目: e-autotest_v1.2.3\"")
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"信息\" msg=\"IP MAC: (11.11.11.86 54-01-4A-F0-88-C4)\"")
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"登录\" msg=\"成功登录: worker\"")
    for _ in range(rng.randint(20, 60)):
        tag = rng.choice(FAKE_TAGS)
        msg = rng.choice(FAKE_MSG_POOL)
        lines.append(f"[{gen_timestamp(rng, base_offset + rng.randint(0, 30))}] INFO tag=\"{tag}\" msg=\"{msg}\"")

    # 第一个 OA3 inject Start 块
    ts_start1 = gen_timestamp(rng, base_offset + 35)
    ts_end1 = gen_timestamp(rng, base_offset + 37)
    lines.append(f"[{ts_start1}] INFO tag=\"跟踪\" msg=\"**********OA3 inject Start...{BASE_DATE} 周二-{ts_start1[11:]}**********\"")
    lines.append(f"[{gen_timestamp(rng, base_offset + 35)}] INFO tag=\"跟踪\" msg=\"<Key><ProductKeyID>{pkid}</ProductKeyID><ProductKeyState>{pk_state}</ProductKeyState><HardwareHash>{hardware_hash}</HardwareHash></Key>\"")
    lines.append(f"[{gen_timestamp(rng, base_offset + 36)}] INFO tag=\"跟踪\" msg=\"SN: {sn}\"")
    # 一些 OA3 注入过程日志
    for _ in range(rng.randint(5, 15)):
        lines.append(f"[{gen_timestamp(rng, base_offset + 35 + rng.random())}] INFO tag=\"跟踪\" msg=\"中间步骤 {rng.randint(1, 99)}\"")
    lines.append(f"[{ts_end1}] INFO tag=\"跟踪\" msg=\"**********OA3 inject End...{BASE_DATE} 周二-{ts_end1[11:]}**********\"")

    # 第二个 OA3 inject Start 块（OA3 每台 2 次）
    ts_start2 = gen_timestamp(rng, base_offset + 40)
    ts_end2 = gen_timestamp(rng, base_offset + 42)
    lines.append(f"[{ts_start2}] INFO tag=\"跟踪\" msg=\"**********OA3 inject Start...{BASE_DATE} 周二-{ts_start2[11:]}**********\"")
    lines.append(f"[{gen_timestamp(rng, base_offset + 40)}] INFO tag=\"跟踪\" msg=\"<Key><ProductKeyID>{pkid}</ProductKeyID><ProductKeyState>{pk_state}</ProductKeyState><HardwareHash>{hardware_hash}</HardwareHash></Key>\"")
    lines.append(f"[{gen_timestamp(rng, base_offset + 41)}] INFO tag=\"跟踪\" msg=\"SN: {sn}\"")
    for _ in range(rng.randint(5, 15)):
        lines.append(f"[{gen_timestamp(rng, base_offset + 40 + rng.random())}] INFO tag=\"跟踪\" msg=\"中间步骤 {rng.randint(1, 99)}\"")
    lines.append(f"[{ts_end2}] INFO tag=\"跟踪\" msg=\"**********OA3 inject End...{BASE_DATE} 周二-{ts_end2[11:]}**********\"")

    # 尾部若干 R<{...}>R 摘要（data_items 来源）+ 收尾 PASS
    for _ in range(rng.randint(3, 6)):
        lines.append(f"[{gen_timestamp(rng, base_offset + 45)}] INFO tag=\"自动测试结果\" msg=\"R<{{{FAKE_TAIL_JSON}}}>R\"")
    lines.append(f"[{gen_timestamp(rng, base_offset + 50)}] INFO tag=\"自动测试\" msg=\"最终 PASS: {rng.randint(180, 200)}; FAIL: 0\"")
    return "\n".join(lines) + "\n"


def gen_fake_eautotest(rng: random.Random, idx: int) -> str:
    """生成一份假 e-autotest 风格日志：不含任何 OA3 锚点。"""
    base_offset = rng.randint(0, TIME_RANGE - 60)
    target_size = rng.randint(1024, 50 * 1024)  # 1KB ~ 50KB

    lines: list[str] = []
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"信息\" msg=\"项目: e-autotest_v1.2.3\"")
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"信息\" msg=\"IP MAC: (11.11.11.86 54-01-4A-F0-88-C4)\"")
    lines.append(f"[{gen_timestamp(rng, base_offset)}] INFO tag=\"登录\" msg=\"成功登录: worker\"")

    while True:
        # 累积直到达到目标大小
        cur_size = sum(len(x) + 1 for x in lines)
        if cur_size >= target_size:
            break
        tag = rng.choice(FAKE_TAGS)
        msg = rng.choice(FAKE_MSG_POOL)
        # 让单行有 5%~15% 的概率写得长一些（贴近真实日志的「失败堆栈 / JSON 摘要」）
        if rng.random() < 0.10:
            msg = msg + " :: " + " ".join(rng.choice(FAKE_MSG_POOL) for _ in range(rng.randint(2, 8)))
        level = rng.choice(["INFO", "INFO", "INFO", "WARN", "ERROR"])
        lines.append(f"[{gen_timestamp(rng, base_offset + rng.randint(0, 50))}] {level} tag=\"{tag}\" msg=\"{msg}\"")

    # 收尾 R<{...}>R 摘要（让 e-autotest 路径的 data_items 也能跑通）
    for _ in range(rng.randint(2, 5)):
        lines.append(f"[{gen_timestamp(rng, base_offset + 55)}] INFO tag=\"自动测试结果\" msg=\"R<{{{FAKE_TAIL_JSON}}}>R\"")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description="10W+ 黄金数据生成器（1万真 + 9万假）")
    parser.add_argument("--root", default=None, help="输出根目录，默认本脚本同目录")
    parser.add_argument("--real", type=int, default=10000, help="真样本数")
    parser.add_argument("--fake", type=int, default=90000, help="假样本数")
    parser.add_argument("--per-dir", type=int, default=1000, help="每个子目录文件数（NTFS 性能考虑）")
    parser.add_argument("--seed", type=int, default=0, help="随机种子（默认 0 = 固定输出可复现）")
    args = parser.parse_args()

    root = Path(args.root) if args.root else Path(__file__).resolve().parent
    rng = random.Random(args.seed)

    real_dir = root / "real"
    fake_dir = root / "fake"
    real_dir.mkdir(parents=True, exist_ok=True)
    fake_dir.mkdir(parents=True, exist_ok=True)

    manifest: list[dict[str, str]] = []

    # ---- 写真样本 ----
    print(f"[gen] 写真样本 {args.real} 份 → {real_dir}", flush=True)
    for i in range(1, args.real + 1):
        batch_idx = (i - 1) // args.per_dir
        sub = real_dir / f"batch_{batch_idx:02d}"
        sub.mkdir(parents=True, exist_ok=True)
        sn = f"MT71I2GSF-2HG260807250XAG{i:04d}"
        path = sub / f"{sn}.log"
        path.write_text(gen_real_oa3(rng, i), encoding="utf-8")
        manifest.append({
            "rel_path": str(path.relative_to(root)).replace("\\", "/"),
            "expected_type": "etest(OA3)",
        })
        if i % 1000 == 0:
            print(f"  real {i}/{args.real}", flush=True)

    # ---- 写假样本 ----
    print(f"[gen] 写假样本 {args.fake} 份 → {fake_dir}", flush=True)
    for i in range(1, args.fake + 1):
        batch_idx = (i - 1) // args.per_dir
        sub = fake_dir / f"batch_{batch_idx:02d}"
        sub.mkdir(parents=True, exist_ok=True)
        path = sub / f"AUTO2_bench_{i:05d}.log"
        path.write_text(gen_fake_eautotest(rng, i), encoding="utf-8")
        manifest.append({
            "rel_path": str(path.relative_to(root)).replace("\\", "/"),
            "expected_type": "e-autotest",
        })
        if i % 5000 == 0:
            print(f"  fake {i}/{args.fake}", flush=True)

    # ---- 写 manifest ----
    manifest_path = root / "manifest.json"
    with manifest_path.open("w", encoding="utf-8") as f:
        json.dump({
            "version": 1,
            "real_count": args.real,
            "fake_count": args.fake,
            "per_dir": args.per_dir,
            "seed": args.seed,
            "files": manifest,
        }, f, ensure_ascii=False, indent=1)
    print(f"[gen] manifest → {manifest_path}（{len(manifest)} 条）", flush=True)

    # ---- 统计 ----
    total_bytes = sum(p.stat().st_size for p in root.rglob("*.log"))
    print(f"[gen] 完成：{len(manifest)} 个文件，总大小 {total_bytes/1024/1024:.1f} MB", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())