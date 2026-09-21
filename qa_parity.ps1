# findany Rust 版 · 移植对拍（Rust vs Python 提取结果逐字段比对 + 批次产物结构）
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File qa_parity.ps1 [-LogDir <dir>]
param([string]$LogDir = "", [string]$Exe = "")

$ErrorActionPreference = "Continue"
$root = $PSScriptRoot
Set-Location $root
$env:PYTHONIOENCODING = "utf-8"
$env:PYTHONWARNINGS = "ignore"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

if ([string]::IsNullOrEmpty($LogDir)) { $LogDir = Join-Path $root "doc\etest-log" }
if ([string]::IsNullOrEmpty($Exe)) { $Exe = Join-Path $root "target\debug\findany.exe" }
if (-not (Test-Path (Join-Path $root "out"))) { New-Item -ItemType Directory -Path (Join-Path $root "out") | Out-Null }

Write-Host "[1/4] Rust extract..."
$rustOut = Join-Path $root "out\_rust_qa.json"
& $Exe --qa $LogDir 2>$null | Out-File -Encoding utf8 -FilePath $rustOut
if ($LASTEXITCODE -ne 0) { Write-Host "  rust run failed"; exit 1 }

Write-Host "[2/4] Python extract (reference)..."
$pyOut = Join-Path $root "out\_py_qa.json"
py -3 "tests\qa_reference.py" $LogDir $pyOut
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Host "[3/4] field-by-field parity..."
py -3 "tests\qa_compare.py" $rustOut $pyOut
$rc = $LASTEXITCODE

Write-Host "[4/4] artifacts structure..."
$batchRoot = Join-Path $env:TEMP "findany-qa-out"
Remove-Item -Recurse -Force $batchRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $batchRoot | Out-Null
$batch = & $Exe --qa-filter $LogDir $batchRoot 2>$null | Select-Object -Last 1
if ([string]::IsNullOrEmpty($batch) -or -not (Test-Path $batch)) {
  Write-Host "  FAIL  批次产物未生成 ($batch)"
  $rc = 1
} else {
  py -3 "tests\qa_artifacts.py" $batch $rustOut
  if ($LASTEXITCODE -ne 0) { $rc = 1 }
}
exit $rc
