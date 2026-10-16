# Tauri 外壳的本地门禁（CI 与 `just` 都调它，保证"本地跑的"和"CI 跑的"是同一件事）。
#
# 默认跑（都不需要图形界面，几分钟内完成）：
#   1. `cargo build --manifest-path tauri/Cargo.toml`（顺带把 rslib 一起编出来）
#   2. `cargo test --manifest-path tauri/Cargo.toml`
#   3. `cargo clippy --manifest-path tauri/Cargo.toml --all-targets`（**不**加 `-D warnings`：
#      仓库里还有一批历史告警，见 PARITY 附注）
#   4. `corepack yarn build`（产出 `out/sveltekit`，一键脚本都依赖它）
#   5. `eslint --max-warnings=0 -c .eslintrc.cjs ts`
#   6. `corepack yarn svelte-check:once`
#
# 可选：
#   -SkipBuild          跳过 1 与 4（只想跑测试/lint 时）
#   -RunVerify          再跑**全部** `tauri/tools/verify-*.ps1`（需要图形界面，一个个跑，
#                       共享 `out/tauri-test/collection.anki2`，所以必须串行）
#   -Verify a,b         只跑指定的几个（名字不带 `verify-` 前缀，例如 `-Verify decks,sync`）
#
# 用法：
#   pwsh -File tauri/tools/check-all.ps1
#   pwsh -File tauri/tools/check-all.ps1 -SkipBuild
#   pwsh -File tauri/tools/check-all.ps1 -RunVerify

[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$RunVerify,
    [string[]]$Verify = @(),
    [string]$OutDir = "out/check-all"
)

$ErrorActionPreference = "Continue"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "check-all.log"
$results = [System.Collections.Generic.List[object]]::new()

function Invoke-Step {
    param([string]$Name, [scriptblock]$Action)
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    $started = Get-Date
    & $Action 2>&1 | Tee-Object -FilePath $logPath -Append | ForEach-Object { $_ }
    $code = $LASTEXITCODE
    if ($null -eq $code) { $code = 0 }
    $seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    $ok = ($code -eq 0)
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $ok; Seconds = $seconds; Exit = $code })
    if ($ok) {
        Write-Host "--- $Name 通过（${seconds}s）" -ForegroundColor Green
    } else {
        Write-Host "--- $Name 失败（exit=$code，${seconds}s）" -ForegroundColor Red
    }
}

if (Test-Path $logPath) { Remove-Item $logPath -Force }

if (-not $SkipBuild) {
    Invoke-Step "cargo build（外壳 + rslib）" { cargo build --manifest-path tauri/Cargo.toml }
}
Invoke-Step "cargo test" { cargo test --manifest-path tauri/Cargo.toml }
Invoke-Step "cargo clippy（不加 -D warnings）" { cargo clippy --manifest-path tauri/Cargo.toml --all-targets }
if (-not $SkipBuild) {
    Invoke-Step "yarn build（out/sveltekit）" { corepack yarn build }
}
Invoke-Step "eslint ts" { & node_modules\.bin\eslint.cmd --max-warnings=0 -c .eslintrc.cjs ts }
Invoke-Step "svelte-check" { corepack yarn svelte-check:once }

$verifyNames = @()
if ($RunVerify) {
    $verifyNames = Get-ChildItem (Join-Path $root "tauri/tools") -Filter "verify-*.ps1" |
        Sort-Object Name | ForEach-Object { $_.BaseName -replace "^verify-", "" }
} elseif ($Verify.Count -gt 0) {
    $verifyNames = $Verify
}
foreach ($name in $verifyNames) {
    $script = Join-Path $root "tauri/tools/verify-$name.ps1"
    if (-not (Test-Path $script)) {
        Write-Host "`n=== 跳过 verify-$name（没有这个脚本）" -ForegroundColor Yellow
        continue
    }
    Invoke-Step "verify-$name" { & pwsh -File $script }
}

Write-Host "`n=== 汇总 ===" -ForegroundColor Cyan
foreach ($result in $results) {
    $mark = if ($result.Ok) { "PASS" } else { "FAIL" }
    Write-Host ("{0}  {1}（{2}s）" -f $mark, $result.Name, $result.Seconds)
}
$failed = @($results | Where-Object { -not $_.Ok })
Write-Host "`n日志：$logPath"
if ($failed.Count -gt 0) {
    Write-Host "$($failed.Count) 项失败：$($failed.Name -join '、')" -ForegroundColor Red
    exit 1
}
Write-Host "全部通过（$($results.Count) 项）" -ForegroundColor Green
exit 0
