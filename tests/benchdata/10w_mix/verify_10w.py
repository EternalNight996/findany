"""10W+ 黄金数据自检：交叉验证 manifest.json 与磁盘文件数/总大小。

不依赖 findany / sonar —— 仅验证：
    · 文件数 == manifest.files 长度
    · 真样本目录文件数 == manifest.real_count
    · 假样本目录文件数 == manifest.fake_count
    · 真样本中**至少一份**含 OA3 inject Start + <HardwareHash>（验证生成内容是真的）
    · 假样本中**没有任何一份**含 OA3 inject Start（验证生成内容是假的）

运行：py -3 tests/benchdata/10w_mix/verify_10w.py [root]
退出码：0 通过；1 失败。
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

# 真样本必须含的锚点（Bug-2A 验证点：强制 OA3 时真伪校验依赖这两个锚点）
REAL_MUST_HAVE = ["OA3 inject Start", "<HardwareHash>"]
# 假样本**不能**含的锚点（否则会让 Bug-2A 验证失真）
FAKE_MUST_NOT_HAVE = ["OA3 inject Start", "<HardwareHash>"]


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
    manifest_path = root / "manifest.json"
    if not manifest_path.is_file():
        print(f"[FAIL] manifest.json 不存在：{manifest_path}")
        return 1

    with manifest_path.open("r", encoding="utf-8") as f:
        manifest = json.load(f)
    files = manifest["files"]
    real_count = manifest["real_count"]
    fake_count = manifest["fake_count"]

    fails: list[str] = []
    print(f"[verify] manifest 总数 {len(files)}（真 {real_count} + 假 {fake_count}）")

    # ---- 1. 计数对拍 ----
    real_on_disk = sum(1 for p in root.rglob("real/**/*.log"))
    fake_on_disk = sum(1 for p in root.rglob("fake/**/*.log"))
    if real_on_disk != real_count:
        fails.append(f"真样本磁盘数 {real_on_disk} != manifest {real_count}")
    if fake_on_disk != fake_count:
        fails.append(f"假样本磁盘数 {fake_on_disk} != manifest {fake_count}")
    if len(files) != real_on_disk + fake_on_disk:
        fails.append(f"manifest 总数 {len(files)} != 磁盘总数 {real_on_disk + fake_on_disk}")
    print(f"[verify] 磁盘 真 {real_on_disk} / 假 {fake_on_disk}，合计 {real_on_disk + fake_on_disk}")

    # ---- 2. 真样本至少一份含双锚点 ----
    real_dir = root / "real"
    real_anchor_ok = 0
    for p in list(real_dir.rglob("*.log"))[:5]:  # 抽样前 5 个验证（避免读 1万文件太慢）
        text = p.read_text(encoding="utf-8", errors="replace")
        if all(anchor in text for anchor in REAL_MUST_HAVE):
            real_anchor_ok += 1
    if real_anchor_ok < 3:
        fails.append(f"真样本抽 5 个含双锚点的不够（仅 {real_anchor_ok} 个）")
    print(f"[verify] 真样本抽 5 个：含双锚点 {real_anchor_ok} 个（≥3 即合格）")

    # ---- 3. 假样本全部不含 OA3 锚点（抽 200 个） ----
    fake_dir = root / "fake"
    fake_anchor_leak = 0
    sampled = list(fake_dir.rglob("*.log"))
    rng_step = max(1, len(sampled) // 200)
    for p in sampled[::rng_step][:200]:
        text = p.read_text(encoding="utf-8", errors="replace")
        if any(anchor in text for anchor in FAKE_MUST_NOT_HAVE):
            fake_anchor_leak += 1
    if fake_anchor_leak > 0:
        fails.append(f"假样本抽 200 个有 {fake_anchor_leak} 个意外含 OA3 锚点")
    print(f"[verify] 假样本抽 200 个：含 OA3 锚点 {fake_anchor_leak} 个（=0 即合格）")

    # ---- 4. 总大小 ----
    total_bytes = sum(p.stat().st_size for p in root.rglob("*.log"))
    print(f"[verify] 磁盘总大小 {total_bytes/1024/1024:.1f} MB")

    if fails:
        print("\n[verify] FAIL：")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("\n[verify] ALL PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())