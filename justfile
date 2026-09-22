# ============================================================
# findany 构建 / 打包 / 验证命令 (just)
#
# 【新电脑一键部署】只需 3 步:
#   1. cargo install just          (Rust 未装先装: https://rustup.rs)
#   2. just doctor                 # 自检工具链(缺什么报什么)
#   3. just run                    # 本机跑 GUI
#   发布: just release             # 构建 + 自检 + 对拍 + 打 dist/findany-v<版本>.zip
#
# 参考 etest/justfile 的同一套骨架(统一变量区 / group 分组 / PowerShell shell),
# 按 findany 的实际需要裁剪:无 MSVC 前缀、无 MSI、无 VM、无 features 全量包。
#
# 说明:
#   - Windows 统一走 PowerShell(系统自带),不依赖 Git Bash/cmd 脚本。
#   - Linux 包暂用本机(或 Linux 机)原生 cargo build；交叉编译需先给 Cargo.toml
#     打开 etest 同款 eframe EGL patch，见 patch-linux 提示。
# ============================================================
set windows-shell := ["powershell", "-NoProfile", "-Command"]

# ---- 统一变量区(改这里即可,勿散落硬编码)----
bin       := "findany"                       # crate 名(产物名随它变)
profile   := "release"                       # release / debug
dist_dir  := "dist"                          # 打包输出目录
fixtures  := "doc/etest-log"                 # 对拍/自检用的生产样例日志
win_exe   := "target/" + profile + "/" + bin + ".exe"
linux_bin := "target/" + profile + "/" + bin
cli_exe   := "doc/devicehashupload/intunehelper_cli.exe"   # 回传 CLI(交付材料,不入库)
# WiX Toolset v3 常见安装路径(bin 常不在 PATH,candle/light 靠它定位;缺了 ensure-wix 会提示安装)
wix_bin   := "C:/Program Files (x86)/WiX Toolset v3.14/bin"
linux_triple := "x86_64-unknown-linux-gnu"    # deb 打包用的 Linux 目标(与 etest 同款)

# findany.toml 默认模板 —— 与 src/core/logfilter/autoconfig.rs::DEFAULT_TOML 逐字一致(改一处必须改另一处)
toml_template := '''
# findany 配置（唯一配置文件；GUI 与自动化共用，改完保存即生效）

[filter]
root_dir = ""                  # 方案一：SN 检索根目录；必填
log_type = "auto"              # auto | etest(OA3) | etest | e-autotest | 海格旧测试2 | 海格旧测试3
recursive = true
ui_refresh_ms = 200            # 界面实时渲染间隔(ms)：每满这么久推一批给表格；0=只在结束时出结果
name_filter = ""                # 文件名包含（子串，不分大小写）：空=不过滤（只挑名字带某段的文件）
mem_limit_mb = 0               # 内存上限(MB)：超过就主动安全停止（不会无声无息挂掉）；0=自动取物理内存的 90%
batch_dirs = false             # true：按 root_dir 的一级子目录分批跑（百万级目录推荐；内存只跟最大子目录有关）
batch_name_filter = ""          # 分批时只挑名字含这段的子目录（空=全部）
threads = 8                    # 并发线程 1~64：服务器上跑就调小（1~4），别抢生产任务
throttle_ms = 0                # 每批之间的休眠(ms, 0~5000)：给 CPU/磁盘/网络盘让路，服务器上建议 20~200
max_files = 0                  # 最多处理多少个文件(0=不限)：防目录跑飞

[run]
auto_start = false             # 改 true：启动即自动「检测→回传→倒计时关」
countdown_sec = 3              # 完成后倒计时，归零自动关程序
auto_close = true
process_priority = "normal"     # normal | below_normal | idle：服务器上建议 below_normal 或 idle（仅 Windows 生效）

[upload]
enabled = true
dry_run = true                 # 先 true 演练（只组包不打网），无误后改 false
types = ["etest(OA3)"]         # 参与回传的判型
cli_path = ""                  # 空=程序目录下 intunehelper_cli.exe
secret_key = ""                # 正式回传必填；本文件勿提交仓库
args = "upload --stdin --secret-key ~secret_key~"
timeout_sec = 60
max_retries = 3
'''
default:
    @just --list
    @Write-Host ''
    @Write-Host '=== 验证(最常用) ==='
    @Write-Host '  just selftest   端到端自检(69 条断言,对 doc/etest-log 生产样例)'
    @Write-Host '  just parity     移植对拍(Rust vs v1 Python:字段 + 批次产物结构)'
    @Write-Host '  just check      编译检查 + 测试'
    @Write-Host ''
    @Write-Host '=== 运行 ==='
    @Write-Host '  just run         本机 GUI'
    @Write-Host '  just run-auto    无窗口自动化(TOML,产线无人值守)'
    @Write-Host '  just run-qa      跑一遍筛选并打印批次目录'
    @Write-Host ''
    @Write-Host '=== 发布 ==='
    @Write-Host '  just dist        Windows 包 -> dist/findany-v<版本>.zip'
    @Write-Host '  just release     构建 + 自检 + 对拍 + 打包(最常用)'
    @Write-Host ''
    @Write-Host '首次使用先跑: just doctor'

# ---- 辅助(私有,内部复用) ----

# 打印单个构建产物
[private]
artifact path:
    @Get-Item {{path}} | Select-Object FullName, @{n='MB';e={[math]::Round($_.Length/1MB,2)}}

# 生成 findany.toml 默认模板到 target/packaging/(与 src/core/logfilter/autoconfig.rs 的 DEFAULT_TOML 同源)
[private]
gen-toml:
    @New-Item -ItemType Directory -Force -Path target/packaging | Out-Null; $t = '{{toml_template}}'; [IO.File]::WriteAllText('target/packaging/findany.toml', $t, (New-Object System.Text.UTF8Encoding($false)))

# 复制分发包资源:文档 + findany.toml 模板(+ intunehelper_cli.exe 有则带)
[private]
pack-resources dest: gen-toml
    @$d = '{{dest}}'; New-Item -ItemType Directory -Force -Path $d | Out-Null; foreach ($f in @('README.md','LICENSE')) { if (Test-Path $f) { Copy-Item $f $d -Force; Write-Host ('  + ' + $f) } }; Copy-Item 'target/packaging/findany.toml' $d -Force; Write-Host '  + findany.toml'; if (Test-Path '{{cli_exe}}') { Copy-Item '{{cli_exe}}' $d -Force; Write-Host '  + intunehelper_cli.exe' } else { Write-Host '  [提示] 未找到 {{cli_exe}},分发包含回传功能时需自行放入' }; if (Test-Path 'assets') { Copy-Item 'assets' $d -Recurse -Force; Write-Host '  + assets/(含 icon.png 供运行时窗口图标兜底)' }

# 生成 deb 打包资源:启动器 + 桌面入口(骨架照 etest;WriteAllText 用 UTF-8 无 BOM)
# 说明:这行写成单行(不用 just 的 \ 续行),避免 just 报「recipe line has extra leading whitespace」
[private]
gen-packaging: gen-toml
    @New-Item -ItemType Directory -Force -Path target/packaging | Out-Null
    @[IO.File]::WriteAllText('target/packaging/findany-wrapper.sh', [string]::Join([char]10, @('#!/bin/sh','# findany 启动器:安装目录(/usr/lib/findany)只读,配置/日志/产物必须落到用户目录','# FINDANY_HOME 会被 src/core/app_dir.rs 当作「程序目录」','DIR="${FINDANY_HOME:-$HOME/.findany}"','mkdir -p "$DIR" || exit 1','[ -f "$DIR/findany.toml" ] || cp /usr/lib/findany/findany.toml "$DIR/findany.toml" 2>/dev/null || true','export FINDANY_HOME="$DIR"','exec /usr/lib/findany/findany.bin "$@"')))
    @[IO.File]::WriteAllText('target/packaging/findany.desktop', [string]::Join([char]10, @('[Desktop Entry]','Type=Application','Name=findany','Name[zh_CN]=findany 日志筛选回传','Comment=Directory scanner / production log filter','Comment[zh_CN]=目录内容扫描器 / 产线日志筛选回传','Exec=findany','Terminal=false','Categories=Utility;')))

# cargo-wix + WiX Toolset 检查(msi 复用;与 etest 同款提示)
[private]
ensure-wix:
    @if (-not (Get-Command cargo-wix -ErrorAction SilentlyContinue)) { Write-Host '缺少 cargo-wix: cargo install cargo-wix'; exit 1 }
    @if (-not (Test-Path '{{wix_bin}}/candle.exe') -and -not (Get-Command candle.exe -ErrorAction SilentlyContinue)) { Write-Host '缺少 WiX Toolset v3(candle/light): winget install --id WiXToolset.WiXToolset --accept-package-agreements --accept-source-agreements'; Write-Host '(装完若 bin 不在 PATH,本 justfile 会自动探测 {{wix_bin}})'; exit 1 }

# 对拍/自检的前置:确认样例日志在位
[private]
ensure-fixtures:
    @if (-not (Test-Path '{{fixtures}}')) { Write-Host '缺少样例日志目录 {{fixtures}}(交付材料,不入库;从产线取样本放入后重试)'; exit 1 }

# ---- 环境自检 ----

# 环境自检:分级检查[必需]构建运行 / [可选]交叉编译与对拍
[group('4 环境')]
doctor:
    @Write-Host '=== findany 环境自检 ==='
    @Write-Host '--- [必需] 构建 / 运行 ---'
    @if (Get-Command just -ErrorAction SilentlyContinue) { Write-Host '[OK]   just' } else { Write-Host '[缺] just -> cargo install just' }
    @if (Get-Command cargo -ErrorAction SilentlyContinue) { Write-Host '[OK]   cargo (Rust)' } else { Write-Host '[缺] Rust -> https://rustup.rs' }
    @if (Get-Command rustc -ErrorAction SilentlyContinue) { Write-Host ('[OK]   rustc ' + (rustc --version)) } else { Write-Host '[缺] rustc' }
    @Write-Host '--- [可选] 交叉编译 Linux ---'
    @if (rustup target list --installed 2>$null | Select-String 'x86_64-unknown-linux-gnu') { Write-Host '[OK]   x86_64-unknown-linux-gnu target' } else { Write-Host '[缺] Linux target -> rustup target add x86_64-unknown-linux-gnu' }
    @if (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue) { Write-Host '[OK]   cargo-zigbuild' } else { Write-Host '[缺] cargo-zigbuild -> cargo install cargo-zigbuild(交叉编译 Linux 用)' }
    @if (Get-Command zig -ErrorAction SilentlyContinue) { Write-Host '[OK]   zig (交叉编译器)' } else { Write-Host '[缺] zig -> https://ziglang.org/download/(交叉编译 Linux 用)' }
    @Write-Host '--- [可选] 安装包(msi / deb) ---'
    @if (Get-Command cargo-wix -ErrorAction SilentlyContinue) { Write-Host '[OK]   cargo-wix(MSI 用)' } else { Write-Host '[缺] cargo-wix -> cargo install cargo-wix' }
    @if ((Test-Path '{{wix_bin}}/candle.exe') -or (Get-Command candle.exe -ErrorAction SilentlyContinue)) { Write-Host '[OK]   WiX Toolset v3(candle/light)' } else { Write-Host '[缺] WiX Toolset -> winget install --id WiXToolset.WiXToolset(仅 MSI 用)' }
    @if (Get-Command cargo-deb -ErrorAction SilentlyContinue) { Write-Host '[OK]   cargo-deb(deb 用)' } else { Write-Host '[缺] cargo-deb -> cargo install cargo-deb' }
    @Write-Host '--- [可选] 对拍(需 v1 Python 参照系) ---'
    @if (Get-Command py -ErrorAction SilentlyContinue) { Write-Host '[OK]   py (Python 启动器)' } else { Write-Host '[缺] Python -> 对拍用 py -3 运行 tests/qa_reference.py' }
    @if (Test-Path 'legacy/v1-python/sonar/logfilter/engine.py') { Write-Host '[OK]   legacy/v1-python 参照系' } else { Write-Host '[缺] 参照系 -> git archive HEAD | tar -x -C legacy/v1-python(或跑 just parity-init)' }
    @Write-Host '--- [可选] 交付样例 ---'
    @if (Test-Path '{{fixtures}}') { Write-Host '[OK]   {{fixtures}}(自检/对拍样例)' } else { Write-Host '[缺] {{fixtures}} -> 从产线取 6 份样例日志放入' }
    @if (Test-Path '{{cli_exe}}') { Write-Host '[OK]   intunehelper_cli.exe(真回传用)' } else { Write-Host '[缺] {{cli_exe}} -> 分发含回传功能时需放入' }

# 导出 v1 Python 参照系到 legacy/v1-python(对拍用;不入库,已 gitignore)
[group('4 环境')]
parity-init:
    @if (Test-Path 'legacy/v1-python/sonar/logfilter/engine.py') { Write-Host 'legacy/v1-python 已存在,跳过(如需重建先删目录)' } else { New-Item -ItemType Directory -Force -Path legacy | Out-Null; git archive HEAD -o legacy/v1-python.zip; Expand-Archive -Path legacy/v1-python.zip -DestinationPath legacy/v1-python -Force; Remove-Item -Force legacy/v1-python.zip; Write-Host '参照系就绪: legacy/v1-python/' }

# 读取当前版本号(Cargo.toml 为唯一来源)
[group('4 环境')]
version:
    @(Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# ---- 构建 / 运行 ----

# 本机运行 GUI(debug)
[group('2 构建')]
run:
    cargo run

# Windows release 构建
[group('2 构建')]
build-win:
    cargo build --{{profile}} --quiet
    @just artifact {{win_exe}}

# Linux 本机 release 构建(在 Linux 机上跑;Windows 交叉编译见 Cargo.toml 的 patch-linux 提示)
[group('2 构建')]
build-linux:
    cargo build --{{profile}} --quiet
    @just artifact {{linux_bin}}

# 无窗口自动化:产线无人值守(读程序目录 findany.toml;退出码 0=成功 2=首次生成模板)
[group('2 构建')]
run-auto ARGS="":
    @& 'target/debug/findany.exe' --auto {{ARGS}}; $rc = $LASTEXITCODE; Write-Host ('exit=' + $rc); exit $rc

# 跑一遍筛选并打印批次目录(产物结构对拍用)
[group('2 构建')]
run-qa logdir=fixtures outdir="out/_just_qa":
    @$o = Join-Path $PWD '{{outdir}}'; & 'target/debug/findany.exe' --qa-filter "{{logdir}}" "$o"; $rc = $LASTEXITCODE; Write-Host ('exit=' + $rc); exit $rc

# 提示:交叉编译 Linux 需要 etest 同款 eframe EGL patch
[group('2 构建')]
patch-linux:
    @Write-Host '交叉编译 / 国产 Linux 适配步骤(与 etest 一致):'
    @Write-Host '  1. Cargo.toml 取消注释 [patch.crates-io] eframe 段(gitee fork v0.36.0-egl1)'
    @Write-Host '  2. rustup target add x86_64-unknown-linux-gnu && cargo install cargo-zigbuild'
    @Write-Host '  3. cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.27'
    @Write-Host '  4. 目标机装依赖: libgl1 libegl1 libxcursor1 libxi6 libxrandr2 libxinerama1 libxkbcommon0 libxkbcommon-x11-0'
    @Write-Host '  5. 启动失败时试: E_AUTOTEST_GL=egl ./findany'

# ---- 验证 ----

# 端到端自检(release 二进制,69 条断言:判型/字段/Excel 产物/dry-run 组包)
[group('1 验证')]
selftest: ensure-fixtures
    cargo build --{{profile}} --quiet
    @$p = Start-Process -FilePath '{{win_exe}}' -ArgumentList '--selftest','{{fixtures}}' -Wait -PassThru -NoNewWindow; Write-Host ('exit=' + $p.ExitCode); exit $p.ExitCode

# 编译检查 + 单元测试
[group('1 验证')]
check:
    cargo check
    cargo test --quiet

# 一键全验证:构建检查 + 自检 + 界面虚拟化 + 对拍 + 产物结构
[group('1 验证')]
verify:
    @Write-Host '=== [1/5] 编译检查 + 单元测试 ==='
    @cargo check --quiet; if ($LASTEXITCODE -ne 0) { Write-Host '编译检查失败'; exit 1 }
    @cargo test --quiet; if ($LASTEXITCODE -ne 0) { Write-Host '单元测试失败'; exit 1 }
    @Write-Host '=== [2/5] 端到端自检 ==='
    @cargo build --{{profile}} --quiet; if ($LASTEXITCODE -ne 0) { Write-Host '构建失败'; exit 1 }
    @$p = Start-Process -FilePath '{{win_exe}}' -ArgumentList '--selftest','{{fixtures}}' -Wait -PassThru -NoNewWindow; if ($p.ExitCode -ne 0) { Write-Host ('自检失败 rc=' + $p.ExitCode); exit 1 }
    @Write-Host '=== [3/5] 界面渲染虚拟化 ==='
    @$p = Start-Process -FilePath '{{win_exe}}' -ArgumentList '--uitest' -Wait -PassThru -NoNewWindow; if ($p.ExitCode -ne 0) { Write-Host ('界面自检失败 rc=' + $p.ExitCode); exit 1 }
    @Write-Host '=== [4/5] 移植对拍 + [5/5] 产物结构 ==='
    @powershell -NoProfile -ExecutionPolicy Bypass -File qa_parity.ps1 -Exe (Join-Path (Get-Location) '{{win_exe}}'); if ($LASTEXITCODE -ne 0) { Write-Host '对拍失败'; exit 1 }
    @Write-Host ''
    @Write-Host '全部验证通过'

# 移植对拍:Rust vs v1 Python 逐字段 + 批次产物结构(需 legacy/v1-python 参照系)
[group('1 验证')]
parity: ensure-fixtures parity-init
    cargo build --quiet
    powershell -NoProfile -ExecutionPolicy Bypass -File qa_parity.ps1 2>$null

# 用 release 二进制对拍(发布前跑这个)
[group('1 验证')]
parity-release: ensure-fixtures parity-init build-win
    powershell -NoProfile -ExecutionPolicy Bypass -File qa_parity.ps1 -Exe (Join-Path (Get-Location) '{{win_exe}}')

# ---- 打包 ----

# 打 Windows 包:dist/findany-v<版本>.zip(程序 + findany.toml 模板 + 文档)
[group('3 打包')]
dist: build-win
    @$v = (just version | Select-Object -Last 1).Trim(); $d = '{{dist_dir}}/findany-v' + $v; if (Test-Path $d) { Remove-Item -Recurse -Force $d }; New-Item -ItemType Directory -Force -Path ($d + '/windows') | Out-Null; Copy-Item {{win_exe}} ($d + '/windows/'); just pack-resources ($d + '/windows'); if (Test-Path 'mesa') { $md = $d + '/windows/mesa'; New-Item -ItemType Directory -Force -Path $md | Out-Null; Copy-Item 'mesa/*.dll' $md -Force; Write-Host ('  + windows/mesa/ (软件 OpenGL 兜底: ' + (Get-ChildItem $md -Filter '*.dll').Count + ' 个 dll)') } else { Write-Host '  [提示] 无 mesa/:不含软件 OpenGL 兜底(服务器/RDP 上可能起不来界面)' }; $zip = '{{dist_dir}}/findany-v' + $v + '.zip'; if (Test-Path $zip) { Remove-Item -Force $zip }; Compress-Archive -Path $d -DestinationPath $zip; Write-Host ('打包完成: ' + (Resolve-Path $zip).Path)

# 打 Linux 包:dist/findany-v<版本>-linux.tar.gz(需在 Linux 上先 just build-linux)
[group('3 打包')]
dist-linux:
    @if (-not (Test-Path '{{linux_bin}}')) { Write-Host '未找到 {{linux_bin}}:先在 Linux 上执行 just build-linux'; exit 1 }
    @$v = (just version | Select-Object -Last 1).Trim(); $d = '{{dist_dir}}/findany-v' + $v + '-linux'; if (Test-Path $d) { Remove-Item -Recurse -Force $d }; New-Item -ItemType Directory -Force -Path $d | Out-Null; Copy-Item {{linux_bin}} $d; just pack-resources $d; $tgz = '{{dist_dir}}/findany-v' + $v + '-linux.tar.gz'; if (Test-Path $tgz) { Remove-Item -Force $tgz }; tar -czf $tgz -C {{dist_dir}} ('findany-v' + $v + '-linux'); Write-Host ('打包完成: ' + (Resolve-Path $tgz).Path)

# 打 Windows MSI 安装包(cargo-wix + WiX;首次自动 cargo wix init 生成 wix/main.wxs)
[group('3 打包')]
msi: build-win
    @just ensure-wix
    @if (-not (Test-Path wix/main.wxs)) { cargo wix init }
    $env:PATH = '{{wix_bin}};' + $env:PATH; $bin = 'target/{{profile}}'; $ms = 'mesa'; $dlls = @(Get-ChildItem $ms -Filter '*.dll' -ErrorAction SilentlyContinue); Remove-Item Env:FINDANY_MESA -ErrorAction SilentlyContinue; Remove-Item -Force 'wix/mesa.generated.wxi' -ErrorAction SilentlyContinue; if ($dlls.Count -gt 0) { $md = Join-Path $bin 'mesa'; New-Item -ItemType Directory -Force -Path $md | Out-Null; $dlls | ForEach-Object { Copy-Item $_.FullName $md -Force }; $i = 0; $files = @($dlls | ForEach-Object { $i++; $id = 'mesaFile' + $i + '_' + ($_.BaseName -replace '[^A-Za-z0-9]', '_'); $kp = if ($i -eq 1) { " KeyPath='yes'" } else { '' }; '        <File Id="' + $id + '" Name="' + $_.Name + '" Source="$(var.CargoTargetBinDir)\mesa\' + $_.Name + '"' + $kp + '/>' }); $body = @('<!-- generated by "just msi": dll list of mesa/ (file names vary by Mesa release, so it cannot be hardcoded) -->', '<Include>', '<Directory Id="MesaDir" Name="mesa">', '    <Component Id="MesaGL" Guid="B3D7F204-6A15-4E88-9C42-5F1B8E7A3D60" DiskId="1">') + $files + @('    </Component>', '</Directory>', '</Include>'); [IO.File]::WriteAllLines((Join-Path $PWD 'wix/mesa.generated.wxi'), $body, (New-Object System.Text.UTF8Encoding($false))); $env:FINDANY_MESA = '1'; Write-Host ('  + MSI 含软件 OpenGL 兜底: ' + $dlls.Count + ' 个 dll 装到 bin/mesa/') } else { Write-Host '  [提示] 无 mesa/*.dll:MSI 不含软件 OpenGL 兜底(服务器/RDP 上可能起不来界面)' }; if (Test-Path 'assets/icon.ico') { Copy-Item 'assets/icon.ico' 'wix/icon.ico' -Force; Write-Host '  + MSI 含产品图标' }; cargo wix --nocapture -L "-ice:!ICE38,!ICE43,!ICE57"
    @New-Item -ItemType Directory -Force -Path {{dist_dir}} | Out-Null; Copy-Item target/wix/*.msi {{dist_dir}}/ -Force; Get-ChildItem {{dist_dir}}/*.msi | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,2)}}

# 打 Linux deb 安装包(需先有 Linux 产物:本机 just build-linux,或 cargo zigbuild 交叉)
[group('3 打包')]
deb: gen-packaging
    @if (-not (Test-Path '{{linux_bin}}')) { Write-Host '缺少 Linux 产物 {{linux_bin}}'; Write-Host '  本机 Linux: just build-linux'; Write-Host '  Windows 交叉: cargo install cargo-zigbuild + zig,再 cargo zigbuild --release --target {{linux_triple}}'; exit 1 }
    cargo deb --no-build --no-strip --target {{linux_triple}}
    @$src = if (Test-Path 'target/{{linux_triple}}/debian') { 'target/{{linux_triple}}/debian/*.deb' } else { 'target/debian/*.deb' }; New-Item -ItemType Directory -Force -Path {{dist_dir}} | Out-Null; $f = Get-ChildItem $src -ErrorAction SilentlyContinue; if ($f) { Copy-Item $f {{dist_dir}}/ -Force; Get-ChildItem {{dist_dir}}/*.deb | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,2)}} } else { Write-Host '未找到 deb 产物,检查 cargo-deb 输出'; exit 1 }

# 一键发布:构建 + 自检 + 对拍 + 打包 + MSI(缺 WiX 自动跳过;deb 需 Linux 产物)
[group('3 打包')]
release: build-win selftest parity-release dist
    @if ((Get-Command cargo-wix -ErrorAction SilentlyContinue) -and ((Test-Path '{{wix_bin}}/candle.exe') -or (Get-Command candle.exe -ErrorAction SilentlyContinue))) { just msi } else { Write-Host '[跳过 MSI] 未装 cargo-wix / WiX Toolset' }
    @if (Test-Path '{{linux_bin}}') { just deb } else { Write-Host '[跳过 deb] 没有 Linux 产物(先 just build-linux 或 zigboot 交叉)' }
    @Write-Host ''
    @Write-Host '=== dist/ 全部产物 ==='
    @Get-ChildItem {{dist_dir}} -File | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,2)}}, LastWriteTime

# ---- 清理 ----

# 清理构建产物与打包目录
[group('4 环境')]
clean:
    cargo clean
    @if (Test-Path {{dist_dir}}) { Remove-Item -Recurse -Force {{dist_dir}} }
