# 10W+ 黄金数据（Bug-2A / LRU 缓存压测用）

## 用途
- **真样本**（10K，路径前缀 `real/`）：含 OA3 双锚点（`OA3 inject Start` + `<HardwareHash>`）+ 4000 字符 HardwareHash + SN/PKID 唯一
  - detect 真值 = `etest(OA3)`；强制 `etest(OA3)` 提取走 OA3 路径
- **假样本**（90K，路径前缀 `fake/`）：e-autotest 风格日志，**不含** 任何 OA3 锚点
  - detect 真值 = `e-autotest`；强制 `etest(OA3)` 提取**必须**降级（**Bug-2A 验证点**）

主上：跑 `findany --auto` 或 GUI 加载 `tests/benchdata/10w_mix`，**配置**：
- `root_dir = tests/benchdata/10w_mix`
- `log_type = etest(OA3)`（强制）
- `upload_types = [etest(OA3)]`

**预期结果**：
- `extract ok = 10万`（真+假都成功提取，**没有**"读取失败/超大跳过"）
- `unknown = 0`
- `upload_dry = 1万`（只有真样本走回传；假样本被 Bug-2A 真伪校验挡住）
- 内存稳定（用默认 `cache_capacity_rows = 5000`）

## 生成

```powershell
# 默认：1万真 + 9万假 = 10万份
py -3 tests\benchdata\10w_mix\gen_10w.py

# 跑小一点测一下（比如 100 + 900）
py -3 tests\benchdata\10w_mix\gen_10w.py --root=tests\benchdata\smoke --real=100 --fake=900
```

选项：
- `--root=DIR` 输出根目录
- `--real=N` 真样本数（默认 10000）
- `--fake=N` 假样本数（默认 90000）
- `--per-dir=1000` 每子目录文件数（NTFS 性能考虑，默认 1000）
- `--seed=0` 随机种子（默认 0 = 固定输出可复现）

输出目录结构（10万级）：
```
tests/benchdata/10w_mix/
├── manifest.json
├── real/
│   ├── batch_00/MT71I2GSF-...-0001.log .. MT71I2GSF-...-1000.log
│   ├── batch_01/MT71I2GSF-...-1001.log .. MT71I2GSF-...-2000.log
│   ...
│   └── batch_09/
└── fake/
    ├── batch_00/AUTO2_bench_00001.log .. AUTO2_bench_01000.log
    ...
    └── batch_89/
```

## 自检（不依赖 findany / sonar）

```powershell
py -3 tests\benchdata\10w_mix\verify_10w.py
```

验证项：
- 磁盘文件数 == manifest 文件数
- 真样本抽 5 个，**至少 3 个**含 OA3 双锚点
- 假样本抽 200 个，**必须 0 个**含 OA3 锚点