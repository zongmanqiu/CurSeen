param([switch]$Strict)
$main = Split-Path -Parent $PSScriptRoot
$fail = 0

$p1 = '(^ |[^:])\b(windui|windows)::'
$p2 = 'use windui::ui|use crate::ui'
$p3 = 'use windows::'
$p4 = 'use crate::ui|use crate::modules'

# Rule 1: core must not depend on windui/windows/ui/modules/platform
$dir = Join-Path $main "src/core"
if (Test-Path $dir) {
    $hits = Select-String -Path (Join-Path $dir "*.rs"), (Join-Path $dir "**/*.rs") -Pattern $p1 -ErrorAction SilentlyContinue
    if ($hits) {
        $fail++
        Write-Host "X core layer must not depend on windui/windows/ui/modules/platform" -ForegroundColor Red
        $hits | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber): $($_.Line.Trim())" -ForegroundColor DarkGray }
    }
}

# Rule 2: modules must not depend on ui
$dir = Join-Path $main "src/modules"
if (Test-Path $dir) {
    $hits = Select-String -Path (Join-Path $dir "*.rs"), (Join-Path $dir "**/*.rs") -Pattern $p2 -ErrorAction SilentlyContinue
    if ($hits) {
        $fail++
        Write-Host "X modules layer must not depend on ui" -ForegroundColor Red
        $hits | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber): $($_.Line.Trim())" -ForegroundColor DarkGray }
    }
}

# Rule 3: ui must not use windows directly
$dir = Join-Path $main "src/ui"
if (Test-Path $dir) {
    $hits = Select-String -Path (Join-Path $dir "*.rs"), (Join-Path $dir "**/*.rs") -Pattern $p3 -ErrorAction SilentlyContinue
    if ($hits) {
        $fail++
        Write-Host "X ui layer must not use windows directly" -ForegroundColor Red
        $hits | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber): $($_.Line.Trim())" -ForegroundColor DarkGray }
    }
}

# Rule 4: update must not depend on ui/modules
$dir = Join-Path $main "src/update"
if (Test-Path $dir) {
    $hits = Select-String -Path (Join-Path $dir "*.rs"), (Join-Path $dir "**/*.rs") -Pattern $p4 -ErrorAction SilentlyContinue
    if ($hits) {
        $fail++
        Write-Host "X update layer must not depend on ui/modules" -ForegroundColor Red
        $hits | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber): $($_.Line.Trim())" -ForegroundColor DarkGray }
    }
}

# Extra: non-platform dirs must not use windows::
$viol = Get-ChildItem (Join-Path $main "src") -Recurse -Filter *.rs |
        Where-Object { $_.FullName -notmatch "\\platform\\" } |
        Select-String -Pattern "^\s*use windows::" -ErrorAction SilentlyContinue
if ($viol) {
    $fail++
    Write-Host "X Non-platform dirs must not use windows::" -ForegroundColor Red
    $viol | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber)" -ForegroundColor DarkGray }
}

# New rule: 颜色/尺寸字面量只允许出现在 ui/tokens.rs + core/color.rs
$lit_viol = Get-ChildItem (Join-Path $main "src") -Recurse -Filter *.rs |
        Where-Object { $_.FullName -match "\\.(core|ui)\\." } |
        Select-String -Pattern '(?<![a-zA-Z])(0x[0-9a-fA-F]{2,}|[0-9]+(px|pt|em|em))(?![a-zA-Z])' -ErrorAction SilentlyContinue
if ($lit_viol) {
    $fail++
    Write-Host "X Literal color/size must only appear in ui/tokens.rs or core/color.rs" -ForegroundColor Red
    $lit_viol | ForEach-Object { Write-Host "    $($_.Path):$($_.LineNumber): $($_.Line.Trim())" -ForegroundColor DarkGray }
}

if ($fail -gt 0 -and $Strict) { exit 1 }
Write-Host "Dependency direction check passed." -ForegroundColor Green
