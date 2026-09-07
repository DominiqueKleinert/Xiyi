param(
    [ValidateSet('all', 'pass', 'fail')]
    [string]$Expect = 'all',

    [string[]]$Name,          # 支持通配，如 *loop*，匹配相对 Tests/ 的路径
    [string[]]$Tag,           # 匹配源码头 // @tag a, b 或 list.json 里的 tag
    [switch]$SkipBuild,
    [switch]$Release,
    [switch]$FailFast,
    [switch]$UpdateGoldens,   # 把本次实际输出写回 .out / .err，然后自己 git diff 审查
    [int]$Jobs = 1,
    [int]$TimeoutSeconds = 30,
    [int]$KeepLogs = 20
)

# ===== CI 环境下关闭彩色转义序列 =====
if ($env:CI -and $PSStyle) {
    $PSStyle.OutputRendering = 'PlainText'
}

# ===== 路径发现：不假设固定目录层级深度 =====
# 从脚本所在目录开始向上找，直到某一级目录"同时"含有 Tests/ 和 Standard/
# 两个子目录为止，把它当作工作区根。目录以后再怎么挪、再套一层，
# 只要这两个目录始终是同级关系，这套发现逻辑就不用改。
function Find-WorkspaceRoot {
    param([string]$StartDir)
    $dir = Get-Item -LiteralPath $StartDir
    while ($dir) {
        $testsPath = Join-Path $dir.FullName 'Tests'
        $stdPath   = Join-Path $dir.FullName 'Standard'
        if ((Test-Path $testsPath) -and (Test-Path $stdPath)) {
            return $dir.FullName
        }
        $parentPath = Split-Path $dir.FullName -Parent
        if (-not $parentPath -or $parentPath -eq $dir.FullName) { return $null }
        $dir = Get-Item -LiteralPath $parentPath
    }
    return $null
}

# 可执行文件不写死 .exe：Windows 下叫 xiyi.exe，Linux/macOS 下叫 xiyi，
# 统一在 target/<profile>/ 下按文件名搜。
function Find-XiyiExe {
    param([string]$CompilerRoot, [bool]$UseRelease)
    $profileName = if ($UseRelease) { 'release' } else { 'debug' }
    $targetDir = Join-Path (Join-Path $CompilerRoot 'target') $profileName
    if (-not (Test-Path $targetDir)) { return $null }
    $candidate = Get-ChildItem -Path $targetDir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq 'xiyi' -or $_.Name -eq 'xiyi.exe' } |
        Select-Object -First 1
    if ($candidate) { return $candidate.FullName }
    return $null
}

# 版本号从 Cargo.toml 读，不在脚本里写死。
function Get-XiyiVersion {
    param([string]$CompilerRoot)
    $cargoToml = Join-Path $CompilerRoot 'Cargo.toml'
    if (Test-Path $cargoToml) {
        $content = Get-Content -Raw $cargoToml
        if ($content -match '(?m)^\s*version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }
    return 'unknown'
}

$COMPILER_ROOT  = $PSScriptRoot
$WORKSPACE_ROOT = Find-WorkspaceRoot -StartDir $COMPILER_ROOT
if (-not $WORKSPACE_ROOT) {
    Write-Host "❌ 从 $COMPILER_ROOT 向上找不到同时包含 Tests/ 与 Standard/ 的工作区根目录" -ForegroundColor Red
    exit 1
}

$STDLIB         = Join-Path $WORKSPACE_ROOT 'Standard'
$TEST_DIR       = Join-Path $WORKSPACE_ROOT 'Tests'
$TEST_LIST_JSON = Join-Path $TEST_DIR 'list.json'

# ===== 日志目录 + 轮转，只保留最近 N 次 =====
$LOG_DIR = Join-Path $COMPILER_ROOT 'test_results'
New-Item -ItemType Directory -Force -Path $LOG_DIR | Out-Null
$runId     = Get-Date -Format 'yyyyMMdd_HHmmss'
$LOG_FILE  = Join-Path $LOG_DIR "$runId.log"
$JSON_FILE = Join-Path $LOG_DIR "$runId.json"

Get-ChildItem -Path $LOG_DIR -Filter '*.log' -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -Skip $KeepLogs |
    Remove-Item -Force -ErrorAction SilentlyContinue
Get-ChildItem -Path $LOG_DIR -Filter '*.json' -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -Skip $KeepLogs |
    Remove-Item -Force -ErrorAction SilentlyContinue

$xiyiVersion = Get-XiyiVersion -CompilerRoot $COMPILER_ROOT

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 希夷编译器测试套件" -ForegroundColor Cyan
Write-Host " xiyi-compiler 版本: $xiyiVersion" -ForegroundColor Cyan
Write-Host " Expect 过滤: $Expect   并发: $Jobs   超时(默认): ${TimeoutSeconds}s" -ForegroundColor Cyan
Write-Host " 编译器目录: $COMPILER_ROOT" -ForegroundColor DarkGray
Write-Host " 标准库目录: $STDLIB" -ForegroundColor DarkGray
Write-Host " 测试目录:   $TEST_DIR" -ForegroundColor DarkGray
Write-Host " 日志:       $LOG_FILE" -ForegroundColor DarkGray
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

"运行 ID: $runId`n版本: $xiyiVersion`n参数: Expect=$Expect Jobs=$Jobs SkipBuild=$SkipBuild Release=$Release UpdateGoldens=$UpdateGoldens FailFast=$FailFast" |
    Out-File -FilePath $LOG_FILE -Encoding utf8

# ===== 步骤1：编译（可跳过） =====
if (-not $SkipBuild) {
    Write-Host "[1/4] 编译 xiyi-compiler..." -ForegroundColor Yellow
    Push-Location $COMPILER_ROOT
    try {
        $buildArgs = @('build', '-q')
        if ($Release) { $buildArgs += '--release' }
        # 关键：先把输出收进变量，再统一写日志。
        # cargo build 2>&1 | Tee-Object 这种写法在 Windows PowerShell 5.1 上
        # 经常让 $LASTEXITCODE 被管道中间的 cmdlet 污染，必须在 cargo
        # 命令结束后、任何其他命令执行前，第一时间读取 $LASTEXITCODE。
        $buildOutput = & cargo @buildArgs 2>&1 | Out-String
        $buildExitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $buildOutput | Out-File -FilePath $LOG_FILE -Append -Encoding utf8
    if ($buildExitCode -ne 0) {
        Write-Host $buildOutput
        Write-Host "❌ 编译失败，停止测试" -ForegroundColor Red
        exit 1
    }
    Write-Host "✅ 编译成功" -ForegroundColor Green
} else {
    Write-Host "[1/4] 跳过编译（-SkipBuild）" -ForegroundColor Yellow
}
Write-Host ""

# ===== 步骤2：定位可执行文件与标准库 =====
$XIYI_EXE = Find-XiyiExe -CompilerRoot $COMPILER_ROOT -UseRelease:$Release
if (-not $XIYI_EXE) {
    $profileName = if ($Release) { 'release' } else { 'debug' }
    Write-Host "❌ 在 target/$profileName 下找不到 xiyi 可执行文件（xiyi 或 xiyi.exe）" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path $STDLIB)) {
    Write-Host "❌ 找不到标准库目录 $STDLIB" -ForegroundColor Red
    exit 1
}
Write-Host "使用可执行文件: $XIYI_EXE" -ForegroundColor DarkGray
Write-Host ""

# ===== 步骤3：递归扫描 Tests/**/*.xiyi，list.json 只作为例外覆盖 =====
Write-Host "[2/4] 扫描测试用例（$TEST_DIR）..." -ForegroundColor Yellow

$allTestFiles = Get-ChildItem -Path $TEST_DIR -Filter '*.xiyi' -File -Recurse |
    Sort-Object FullName

if ($allTestFiles.Count -eq 0) {
    Write-Host "❌ $TEST_DIR 下找不到任何 .xiyi 文件" -ForegroundColor Red
    exit 1
}

$overrides = @{}
# list.json 的项目级默认值：优先级高于脚本的硬编码兜底值（'PASS' / -TimeoutSeconds），
# 因为这是随代码库走的约定，而不是执行者当次敲命令时的临时参数。
$baseExpect  = 'PASS'
$baseTimeout = $TimeoutSeconds
if (Test-Path $TEST_LIST_JSON) {
    try {
        $listJson = Get-Content -Raw -Encoding UTF8 $TEST_LIST_JSON | ConvertFrom-Json

        if ($null -ne $listJson.version -and $listJson.version -ne 1) {
            Write-Host "⚠️  list.json 声明的 version=$($listJson.version)，本脚本只认识 version 1，按 1 处理" -ForegroundColor Yellow
        }

        if ($listJson.defaults) {
            if ($listJson.defaults.expect) {
                $baseExpect = if ($listJson.defaults.expect -in @('FAIL', 'EXPECT_FAIL')) { 'FAIL' } else { 'PASS' }
            }
            if ($listJson.defaults.timeout) {
                $baseTimeout = [int]$listJson.defaults.timeout
            }
        }

        foreach ($t in $listJson.tests) {
            if (-not $t.file) { continue }
            $key = ($t.file -replace '\\', '/')
            $overrides[$key] = $t
        }
    } catch {
        Write-Host "❌ 解析 $TEST_LIST_JSON 失败: $($_.Exception.Message)" -ForegroundColor Red
        exit 1
    }
}

# 源码头内联元数据，例如：
#   // @expect fail
#   // @timeout 5
#   // @stderr-contains "类型不匹配"
#   // @tag parser, typecheck
#   // @skip 尚未实现 break
function Get-InlineMeta {
    param([string]$FilePath, [string]$DefaultExpect, [int]$DefaultTimeout)
    $meta = [ordered]@{
        Expect = $DefaultExpect; Timeout = $DefaultTimeout; StderrContains = $null
        Tags = @(); Skip = $false; Reason = $null
    }
    $lines = Get-Content -Path $FilePath -TotalCount 40 -ErrorAction SilentlyContinue
    foreach ($line in $lines) {
        if ($line -match '^\s*//\s*@expect\s+fail\s*$') {
            $meta.Expect = 'FAIL'
        } elseif ($line -match '^\s*//\s*@timeout\s+(\d+)\s*$') {
            $meta.Timeout = [int]$matches[1]
        } elseif ($line -match '^\s*//\s*@stderr-contains\s+"([^"]*)"\s*$') {
            $meta.StderrContains = $matches[1]
        } elseif ($line -match '^\s*//\s*@tag\s+(.+)$') {
            $meta.Tags = @($matches[1] -split '[,\s]+' | Where-Object { $_ })
        } elseif ($line -match '^\s*//\s*@skip\b\s*(.*)$') {
            $meta.Skip = $true
            $meta.Reason = $matches[1]
        }
    }
    return $meta
}

$testDirNorm = $TEST_DIR.TrimEnd('\', '/')
$testCases = @()
foreach ($f in $allTestFiles) {
    $rel = $f.FullName.Substring($testDirNorm.Length).TrimStart('\', '/') -replace '\\', '/'
    # 优先级（低到高）：list.json 的 defaults 块 -> 源码内联注释 -> list.json 里
    # 针对这个文件的具体例外条目。例外条目永远是"最后一句话"。
    $meta = Get-InlineMeta -FilePath $f.FullName -DefaultExpect $baseExpect -DefaultTimeout $baseTimeout

    if ($overrides.ContainsKey($rel)) {
        $ov = $overrides[$rel]
        if ($ov.expect) {
            $meta.Expect = if ($ov.expect -in @('FAIL', 'EXPECT_FAIL')) { 'FAIL' } else { 'PASS' }
        }
        if ($null -ne $ov.timeout) { $meta.Timeout = [int]$ov.timeout }
        if ($ov.stderrContains) { $meta.StderrContains = $ov.stderrContains }
        if ($ov.tag) { $meta.Tags = @($ov.tag) }
        if ($ov.skip) { $meta.Skip = $true; $meta.Reason = $ov.reason }
    }

    $testCases += [pscustomobject]@{
        RelPath        = $rel
        FullPath       = $f.FullName
        Expect         = $meta.Expect
        Timeout        = $meta.Timeout
        StderrContains = $meta.StderrContains
        Tags           = $meta.Tags
        Skip           = $meta.Skip
        Reason         = $meta.Reason
        OutGolden      = "$($f.FullName).out"
        ErrGolden      = "$($f.FullName).err"
        ExitGolden     = "$($f.FullName).exit"
    }
}
# 目录扫描即清单：不在 list.json 里也会被发现，"孤儿文件默默不跑"的问题不存在了。
# 副作用见下方运行结果——以前的孤儿文件这次真的会被执行。

# ===== 过滤 =====
$filtered = $testCases
if ($Expect -ne 'all') {
    $want = if ($Expect -eq 'pass') { 'PASS' } else { 'FAIL' }
    $filtered = $filtered | Where-Object { $_.Expect -eq $want }
}
if ($Name) {
    $filtered = $filtered | Where-Object {
        $item = $_
        @($Name | Where-Object { $item.RelPath -like $_ }).Count -gt 0
    }
}
if ($Tag) {
    $filtered = $filtered | Where-Object {
        $item = $_
        @($Tag | Where-Object { $item.Tags -contains $_ }).Count -gt 0
    }
}

if (-not $filtered -or $filtered.Count -eq 0) {
    Write-Host "❌ 过滤后没有匹配的测试用例" -ForegroundColor Red
    exit 1
}

Write-Host "共 $($filtered.Count) 个测试用例（目录下总计发现 $($testCases.Count) 个）"
Write-Host ""

# ===== 核心执行：进程重定向 + 异步读 + 超时杀进程树 =====
$InvokeXiyiTest = {
    param($Exe, [string[]]$ArgList, [int]$TimeoutMs)

    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Exe
    # 用字符串拼接而不是 ArgumentList 集合，兼容 Windows PowerShell 5.1
    # （.NET Framework 上 ArgumentList 支持不稳定）。
    $psi.Arguments = ($ArgList | ForEach-Object {
        if ($_ -match '\s') { '"' + $_ + '"' } else { $_ }
    }) -join ' '
    $psi.UseShellExecute        = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError  = $true
    $psi.CreateNoWindow         = $true
    $psi.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding  = [Text.UTF8Encoding]::new($false)

    $p = [System.Diagnostics.Process]::new()
    $p.StartInfo = $psi
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p.Start() | Out-Null
    $null = $p.Handle   # 沿用之前踩过的坑：异步等待路径下必须先摸一次 Handle

    # 异步读 stdout/stderr，避免子进程把管道缓冲区写满导致死锁
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $errTask = $p.StandardError.ReadToEndAsync()

    if (-not $p.WaitForExit($TimeoutMs)) {
        try {
            $p.Kill($true)   # .NET Core 3+：跨平台杀整个进程树
        } catch {
            if ($env:OS -eq 'Windows_NT') {
                try { & taskkill /PID $p.Id /T /F 2>$null | Out-Null } catch {}
            } else {
                try { $p.Kill() } catch {}
            }
        }
        $p.WaitForExit()
        return [pscustomobject]@{
            Timeout = $true; ExitCode = $null
            Stdout = $outTask.Result; Stderr = $errTask.Result
            Elapsed = $sw.Elapsed.TotalSeconds
        }
    }
    [void]$outTask.Wait()
    [void]$errTask.Wait()
    return [pscustomobject]@{
        Timeout = $false; ExitCode = $p.ExitCode
        Stdout = $outTask.Result; Stderr = $errTask.Result
        Elapsed = $sw.Elapsed.TotalSeconds
    }
}

function Format-Normalized {
    param([string]$Text)
    if ($null -eq $Text) { return "`n" }
    return (($Text -replace "`r`n", "`n" -replace "`r", "`n").TrimEnd() + "`n")
}
$NormalizeFnDef = ${function:Format-Normalized}

# 单个用例的判定逻辑，串行/并行共用（作为脚本块传递）
$RunOneCase = {
    param($test, $exe, $stdlib, $updateGoldens, $invokeFn, $normalizeFn)

    if ($test.Skip) {
        return [pscustomobject]@{
            RelPath = $test.RelPath; Ok = $true; Skipped = $true; Updated = $false
            Reasons = @(if ($test.Reason) { "skip: $($test.Reason)" } else { 'skip' })
            ExitCode = $null; Elapsed = 0; Stdout = ''; Stderr = ''
        }
    }

    $result = & $invokeFn -Exe $exe -ArgList @('--stdlib', $stdlib, $test.FullPath) -TimeoutMs ($test.Timeout * 1000)

    if ($updateGoldens -and -not $result.Timeout) {
        Set-Content -Path $test.OutGolden -Value $result.Stdout -Encoding utf8 -NoNewline
        Set-Content -Path $test.ErrGolden -Value $result.Stderr -Encoding utf8 -NoNewline
        return [pscustomobject]@{
            RelPath = $test.RelPath; Ok = $true; Skipped = $false; Updated = $true
            Reasons = @('golden 已更新，请 git diff 审查')
            ExitCode = $result.ExitCode; Elapsed = $result.Elapsed
            Stdout = $result.Stdout; Stderr = $result.Stderr
        }
    }

    $ok = $true
    $reasons = @()

    if ($result.Timeout) {
        $ok = $false
        $reasons += "timeout $($test.Timeout)s"
    } else {
        # 期望退出码：.exit golden 优先，否则按 Expect 推导
        $expectedExit = $null
        if (Test-Path $test.ExitGolden) {
            $expectedExit = [int](Get-Content -Raw $test.ExitGolden).Trim()
        }
        if ($null -ne $expectedExit) {
            if ($result.ExitCode -ne $expectedExit) {
                $ok = $false
                $reasons += "exit $($result.ExitCode), expected $expectedExit"
            }
        } elseif ($test.Expect -eq 'FAIL') {
            if ($result.ExitCode -eq 0) {
                $ok = $false
                $reasons += 'expected failure but exited 0'
            }
        } else {
            if ($result.ExitCode -ne 0) {
                $ok = $false
                $reasons += "exit $($result.ExitCode), expected 0"
            }
        }

        # 有 golden 文件才做精确比对；没有就不强行要求空 stdout/stderr，
        # 避免历史测试因为从没建过 golden 而集体炸红。
        if (Test-Path $test.OutGolden) {
            $golden = Get-Content -Raw $test.OutGolden
            if ((& $normalizeFn $result.Stdout) -ne (& $normalizeFn $golden)) {
                $ok = $false
                $reasons += 'stdout mismatch'
            }
        }
        if (Test-Path $test.ErrGolden) {
            $golden = Get-Content -Raw $test.ErrGolden
            if ((& $normalizeFn $result.Stderr) -ne (& $normalizeFn $golden)) {
                $ok = $false
                $reasons += 'stderr mismatch'
            }
        }
        if ($test.StderrContains -and ($result.Stderr -notmatch [regex]::Escape($test.StderrContains))) {
            $ok = $false
            $reasons += "stderr missing: $($test.StderrContains)"
        }
        if ($test.Expect -eq 'FAIL' -and -not $test.StderrContains -and -not (Test-Path $test.ErrGolden)) {
            $reasons += '(提示: EXPECT_FAIL 未绑定 stderr 校验，通过不代表失败原因正确)'
        }
    }

    [pscustomobject]@{
        RelPath = $test.RelPath; Ok = $ok; Skipped = $false; Updated = $false
        Reasons = $reasons; ExitCode = $result.ExitCode; Elapsed = $result.Elapsed
        Stdout = $result.Stdout; Stderr = $result.Stderr
    }
}

function Write-CaseResult {
    param($r)
    $statusColor = if ($r.Skipped) { 'DarkGray' } elseif ($r.Updated) { 'Magenta' } elseif ($r.Ok) { 'Green' } else { 'Red' }
    $statusText  = if ($r.Skipped) { 'SKIP' } elseif ($r.Updated) { 'UPDATED' } elseif ($r.Ok) { 'PASS' } else { 'FAIL' }
    $suffix = if ($r.Reasons) { ' - ' + ($r.Reasons -join '; ') } else { '' }
    Write-Host ("[{0}] {1} ({2:F2}s){3}" -f $statusText, $r.RelPath, $r.Elapsed, $suffix) -ForegroundColor $statusColor

    if (-not $r.Ok -and -not $r.Skipped) {
        $stdoutTrunc = if ($r.Stdout -and $r.Stdout.Length -gt 4096) { $r.Stdout.Substring(0, 4096) + "`n...(截断)" } else { $r.Stdout }
        $stderrTrunc = if ($r.Stderr -and $r.Stderr.Length -gt 4096) { $r.Stderr.Substring(0, 4096) + "`n...(截断)" } else { $r.Stderr }
        "---- $($r.RelPath) ----`nreasons: $($r.Reasons -join '; ')`nstdout:`n$stdoutTrunc`nstderr:`n$stderrTrunc`n" |
            Out-File -FilePath $LOG_FILE -Append -Encoding utf8
    }
}

# ===== 步骤4：执行 =====
Write-Host "[3/4] 执行测试..." -ForegroundColor Yellow
Write-Host ""

$results = @()

if ($Jobs -gt 1 -and $PSVersionTable.PSVersion.Major -lt 7) {
    Write-Host "⚠️  -Jobs > 1 需要 PowerShell 7+（ForEach-Object -Parallel），当前是 $($PSVersionTable.PSVersion)，回退为串行" -ForegroundColor Yellow
    $Jobs = 1
}

$ranInParallel = $false
if ($Jobs -gt 1) {
    try {
        $parallelResults = $filtered | ForEach-Object -Parallel {
            $runFn = $using:RunOneCase
            & $runFn $_ $using:XIYI_EXE $using:STDLIB $using:UpdateGoldens $using:InvokeXiyiTest $using:NormalizeFnDef
        } -ThrottleLimit $Jobs
        $results = @($parallelResults)
        foreach ($r in $results) { Write-CaseResult $r }
        $ranInParallel = $true
    } catch {
        Write-Host "⚠️  并行执行出错（$($_.Exception.Message)），回退为串行执行" -ForegroundColor Yellow
        $results = @()
    }
}

if (-not $ranInParallel) {
    foreach ($test in $filtered) {
        $r = & $RunOneCase $test $XIYI_EXE $STDLIB $UpdateGoldens $InvokeXiyiTest $NormalizeFnDef
        $results += $r
        Write-CaseResult $r
        if ($FailFast -and -not $r.Ok -and -not $r.Skipped) {
            Write-Host "⏹  -FailFast 触发，停止后续测试" -ForegroundColor Yellow
            break
        }
    }
}
Write-Host ""

# ===== 步骤5：汇总（人读 log + 机器读 json） =====
Write-Host "[4/4] 汇总结果..." -ForegroundColor Yellow

$total        = $results.Count
$skippedCount = @($results | Where-Object { $_.Skipped }).Count
$updatedCount = @($results | Where-Object { $_.Updated }).Count
$passedCount  = @($results | Where-Object { $_.Ok -and -not $_.Skipped -and -not $_.Updated }).Count
$failedCount  = @($results | Where-Object { -not $_.Ok -and -not $_.Skipped }).Count
$timeoutCount = @($results | Where-Object { ($_.Reasons -join ';') -match 'timeout' }).Count

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 测试结果汇总" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 总计:   $total" -ForegroundColor White
Write-Host " 通过:   $passedCount" -ForegroundColor Green
Write-Host " 失败:   $failedCount" -ForegroundColor Red
Write-Host " 超时:   $timeoutCount" -ForegroundColor Yellow
Write-Host " 跳过:   $skippedCount" -ForegroundColor DarkGray
Write-Host " 已更新 golden: $updatedCount" -ForegroundColor Magenta
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 日志:  $LOG_FILE"
Write-Host " 结果:  $JSON_FILE"
Write-Host "========================================" -ForegroundColor Cyan

$summary = [pscustomobject]@{
    RunId    = $runId
    Version  = $xiyiVersion
    Total    = $total
    Passed   = $passedCount
    Failed   = $failedCount
    Timeouts = $timeoutCount
    Skipped  = $skippedCount
    Updated  = $updatedCount
    Results  = $results
}
$summary | ConvertTo-Json -Depth 6 | Out-File -FilePath $JSON_FILE -Encoding utf8

"汇总: 总计 $total, 通过 $passedCount, 失败 $failedCount, 超时 $timeoutCount, 跳过 $skippedCount, 已更新 $updatedCount" |
    Out-File -FilePath $LOG_FILE -Append -Encoding utf8

if ($UpdateGoldens) {
    Write-Host "✅ Golden 更新完成，请务必 git diff 审查改动后再提交" -ForegroundColor Green
    exit 0
}

if ($failedCount -eq 0) {
    Write-Host "✅ 所有测试通过！" -ForegroundColor Green
    exit 0
} else {
    Write-Host "❌ 有 $failedCount 个测试失败" -ForegroundColor Red
    exit 1
}
