param(
    [string]$TestType = "all",
    [int]$TimeoutSeconds = 30
)

# ===== 路径锚点 =====
# 不再依赖"当前工作目录"，一律以脚本自身所在位置为基准。
# 约定：本脚本放在 Projects\xiyi-compiler\ 根目录（与 Cargo.toml 同级）。
# 目录结构：
#   D:\the-xiyi-language\
#   ├── Projects\xiyi-compiler\   <- $COMPILER_ROOT（本脚本所在处）
#   ├── Tests\                    <- 测试用例 + list.json
#   └── Standard\                 <- 标准库
$COMPILER_ROOT  = $PSScriptRoot
$WORKSPACE_ROOT = Split-Path (Split-Path $COMPILER_ROOT -Parent) -Parent

$STDLIB         = Join-Path $WORKSPACE_ROOT "Standard\"
$TEST_DIR       = Join-Path $WORKSPACE_ROOT "Tests\"
$TEST_LIST_JSON = Join-Path $TEST_DIR "list.json"
$LOG_FILE       = Join-Path $COMPILER_ROOT "test_results.log"
$XIYI_EXE       = Join-Path $COMPILER_ROOT "target\debug\xiyi.exe"

$passed = 0
$failed = 0
$expected_fail_passed = 0
$expected_fail_failed = 0
$timeouts = 0
$total = 0

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 希夷编译器测试套件" -ForegroundColor Cyan
Write-Host " 版本: v0.1.9.6_Delta" -ForegroundColor Cyan
Write-Host " 测试类型: $TestType" -ForegroundColor Cyan
Write-Host " 超时: ${TimeoutSeconds}s" -ForegroundColor Cyan
Write-Host " 编译器目录: $COMPILER_ROOT" -ForegroundColor DarkGray
Write-Host " 标准库目录: $STDLIB" -ForegroundColor DarkGray
Write-Host " 测试目录:   $TEST_DIR" -ForegroundColor DarkGray
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ===== 写入日志头 =====
$runId = Get-Date -Format "yyyyMMdd_HHmmss"
"========================================" | Out-File -Append $LOG_FILE
"测试运行 ID: $runId" | Out-File -Append $LOG_FILE
"测试时间: $(Get-Date)" | Out-File -Append $LOG_FILE
"测试类型: $TestType" | Out-File -Append $LOG_FILE
"超时: ${TimeoutSeconds}s" | Out-File -Append $LOG_FILE
"编译器目录: $COMPILER_ROOT" | Out-File -Append $LOG_FILE
"标准库目录: $STDLIB" | Out-File -Append $LOG_FILE
"测试目录: $TEST_DIR" | Out-File -Append $LOG_FILE
"========================================" | Out-File -Append $LOG_FILE

# ===== 步骤1：编译编译器 =====
# 显式切到编译器根目录再 build，不依赖脚本调用时人在哪个目录，
# 避免"目录挪了但没人在正确位置运行脚本"导致 cargo 找不到 Cargo.toml。
Write-Host "[1/4] 重新编译编译器..." -ForegroundColor Yellow
Write-Host ""

Push-Location $COMPILER_ROOT
try {
    cargo build 2>&1 | Tee-Object -Append $LOG_FILE
    $buildExitCode = $LASTEXITCODE
} finally {
    Pop-Location
}

if ($buildExitCode -ne 0) {
    Write-Host "❌ 编译失败，停止测试" -ForegroundColor Red
    "❌ 编译失败" | Out-File -Append $LOG_FILE
    exit 1
}

Write-Host ""
Write-Host "✅ 编译成功" -ForegroundColor Green
"✅ 编译成功" | Out-File -Append $LOG_FILE
Write-Host ""

# ===== 步骤2：检查可执行文件与目录 =====
if (-not (Test-Path $XIYI_EXE)) {
    Write-Host "❌ 找不到 $XIYI_EXE" -ForegroundColor Red
    "❌ 找不到 $XIYI_EXE" | Out-File -Append $LOG_FILE
    exit 1
}

if (-not (Test-Path $STDLIB)) {
    Write-Host "❌ 找不到标准库目录 $STDLIB" -ForegroundColor Red
    "❌ 找不到标准库目录 $STDLIB" | Out-File -Append $LOG_FILE
    exit 1
}

if (-not (Test-Path $TEST_LIST_JSON)) {
    Write-Host "❌ 找不到测试列表 $TEST_LIST_JSON" -ForegroundColor Red
    "❌ 找不到测试列表 $TEST_LIST_JSON" | Out-File -Append $LOG_FILE
    exit 1
}

# ===== 步骤3：从 list.json 读取测试列表 =====
Write-Host "[2/4] 加载测试用例（$TEST_LIST_JSON）..." -ForegroundColor Yellow

try {
    $testListRaw = Get-Content -Raw -Encoding UTF8 $TEST_LIST_JSON | ConvertFrom-Json
} catch {
    Write-Host "❌ 解析 list.json 失败: $($_.Exception.Message)" -ForegroundColor Red
    "❌ 解析 list.json 失败: $($_.Exception.Message)" | Out-File -Append $LOG_FILE
    exit 1
}

if (-not $testListRaw.tests -or $testListRaw.tests.Count -eq 0) {
    Write-Host "❌ list.json 中没有任何测试条目" -ForegroundColor Red
    exit 1
}

$parsedTests = @()
foreach ($t in $testListRaw.tests) {
    if (-not $t.file) {
        Write-Host "⚠️  跳过一条缺少 file 字段的记录" -ForegroundColor Yellow
        continue
    }
    $expect = if ($t.expect) { $t.expect } else { "PASS" }
    $parsedTests += [PSCustomObject]@{
        File   = $t.file
        Expect = $expect
    }
}

# 根据 TestType 过滤
if ($TestType -eq "all") {
    $filteredTests = $parsedTests
} elseif ($TestType -eq "pass") {
    $filteredTests = $parsedTests | Where-Object { $_.Expect -eq "PASS" }
} elseif ($TestType -eq "fail") {
    $filteredTests = $parsedTests | Where-Object { $_.Expect -eq "EXPECT_FAIL" }
} else {
    # 按文件名精确匹配
    $filteredTests = $parsedTests | Where-Object { $_.File -eq $TestType }
}

if (-not $filteredTests -or $filteredTests.Count -eq 0) {
    Write-Host "❌ 没有匹配 TestType='$TestType' 的测试用例" -ForegroundColor Red
    exit 1
}

$testListStr = ($filteredTests | ForEach-Object { "$($_.File):$($_.Expect)" }) -join ', '
Write-Host "测试列表: $testListStr"
Write-Host ""

# ===== 提示 Tests 目录下未纳入 list.json 的孤儿文件（不影响本次运行，仅提醒） =====
$allXiyiFiles = Get-ChildItem -Path $TEST_DIR -Filter "*.xiyi" -File -ErrorAction SilentlyContinue |
    ForEach-Object { $_.Name }
$listedFiles = $parsedTests | ForEach-Object { $_.File }
$orphanFiles = $allXiyiFiles | Where-Object { $_ -notin $listedFiles }
if ($orphanFiles) {
    Write-Host "⚠️  以下 .xiyi 文件存在于 Tests 目录但未列入 list.json（本次不会运行）：" -ForegroundColor Yellow
    $orphanFiles | ForEach-Object { Write-Host "    - $_" -ForegroundColor DarkYellow }
    Write-Host ""
}

# ===== 步骤4：运行测试 =====
Write-Host "[3/4] 执行测试用例..." -ForegroundColor Yellow
Write-Host ""

foreach ($test in $filteredTests) {
    $total++
    $testFile = $test.File
    $expect = $test.Expect
    $testFilePath = Join-Path $TEST_DIR $testFile

    Write-Host "----------------------------------------" -ForegroundColor Cyan
    Write-Host "测试: $testFile (期望: $expect)" -ForegroundColor Cyan
    Write-Host "----------------------------------------" -ForegroundColor Cyan

    if (-not (Test-Path $testFilePath)) {
        Write-Host "❌ 找不到测试文件: $testFilePath" -ForegroundColor Red
        "[ERROR] $testFile 找不到文件: $testFilePath" | Out-File -Append $LOG_FILE
        $failed++
        Write-Host ""
        continue
    }

    # ===== 运行测试（带超时），整体包一层 try/catch =====
    # 单个测试哪怕启动/等待过程中意外抛异常，也只记这一条失败，
    # 不会导致整个测试套件中断（脚本级别的 uncaught exception 会终止 foreach）。
    $exitCode = $null
    $isTimeout = $false
    $elapsedSeconds = 0.0

    try {
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        $proc = Start-Process -FilePath $XIYI_EXE `
            -ArgumentList @("--stdlib", $STDLIB, $testFilePath) `
            -PassThru -NoNewWindow

        # ===== 关键修复 =====
        # .NET 的 Process 组件有个老坑：如果不先访问一次 Handle，
        # 之后读 ExitCode 会抛 "process has not exited" 之类的异常。
        # -Wait 模式下 PowerShell 内部路径不同，不会触发；
        # 一旦改成异步 Start-Process + 手动 WaitForExit，就必须先摸一下 Handle。
        $null = $proc.Handle

        $finished = $proc.WaitForExit($TimeoutSeconds * 1000)
        $elapsedSeconds = $stopwatch.Elapsed.TotalSeconds

        if (-not $finished) {
            Write-Host ("⏰ 超时 ({0}s)" -f $TimeoutSeconds) -ForegroundColor Yellow
            try { $proc.Kill() } catch {}
            $proc.WaitForExit()
            $isTimeout = $true
            $timeouts++
            "[TIMEOUT] $testFile 超时 ({0:F2}s)" -f $elapsedSeconds | Out-File -Append $LOG_FILE
        } else {
            $exitCode = $proc.ExitCode
            "[TEST] $testFile exit: $exitCode ({0:F2}s)" -f $elapsedSeconds | Out-File -Append $LOG_FILE
        }
    } catch {
        Write-Host "💥 运行测试时抛出异常: $($_.Exception.Message)" -ForegroundColor Red
        "[ERROR] $testFile 运行异常: $($_.Exception.Message)" | Out-File -Append $LOG_FILE
        $exitCode = $null
    }

    # ===== 判断结果 =====
    if ($isTimeout) {
        # 超时已经计数并记录过了，不再走下面的判断
    } elseif ($null -eq $exitCode) {
        # 启动/等待阶段本身就出了异常，视为失败
        Write-Host "❌ 失败（未能获取退出码）" -ForegroundColor Red
        $failed++
    } elseif ($expect -eq "PASS") {
        if ($exitCode -eq 0) {
            Write-Host ("✅ 通过 ({0:F2}s)" -f $elapsedSeconds) -ForegroundColor Green
            $passed++
            "[PASS] $testFile 通过" | Out-File -Append $LOG_FILE
        } else {
            Write-Host ("❌ 失败 (exit code: {0}, {1:F2}s)" -f $exitCode, $elapsedSeconds) -ForegroundColor Red
            $failed++
            "[FAIL] $testFile 失败 (exit: $exitCode)" | Out-File -Append $LOG_FILE
        }
    } elseif ($expect -eq "EXPECT_FAIL") {
        if ($exitCode -ne 0) {
            Write-Host ("✅ 预期失败，符合预期 (exit code: {0}, {1:F2}s)" -f $exitCode, $elapsedSeconds) -ForegroundColor Green
            $expected_fail_passed++
            "[EXPECT_FAIL_PASS] $testFile 失败符合预期" | Out-File -Append $LOG_FILE
        } else {
            Write-Host ("❌ 预期失败但实际通过了！({0:F2}s)" -f $elapsedSeconds) -ForegroundColor Red
            $expected_fail_failed++
            "[EXPECT_FAIL_FAIL] $testFile 预期失败但通过" | Out-File -Append $LOG_FILE
        }
    }
    Write-Host ""
}

# ===== 汇总 =====
Write-Host "[4/4] 汇总结果..." -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 测试结果汇总" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 总测试: $total" -ForegroundColor White
Write-Host " 通过: $passed" -ForegroundColor Green
Write-Host " 失败: $failed" -ForegroundColor Red
Write-Host " 预期失败且符合预期: $expected_fail_passed" -ForegroundColor Yellow
Write-Host " 预期失败但实际通过: $expected_fail_failed" -ForegroundColor Red
Write-Host " 超时: $timeouts" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Cyan

# ===== 写入日志汇总 =====
"========================================" | Out-File -Append $LOG_FILE
"汇总: 总测试 $total, 通过 $passed, 失败 $failed, 超时 $timeouts" | Out-File -Append $LOG_FILE
"========================================" | Out-File -Append $LOG_FILE

if ($failed -eq 0 -and $expected_fail_failed -eq 0 -and $timeouts -eq 0) {
    Write-Host "✅ 所有测试通过！" -ForegroundColor Green
    exit 0
} else {
    Write-Host "❌ 有 $($failed + $expected_fail_failed + $timeouts) 个测试未通过" -ForegroundColor Red
    exit 1
}
