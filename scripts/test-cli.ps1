param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [switch]$InjectAssertionFailure
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('repomeow-shell-' + [Guid]::NewGuid().ToString('N'))
$originalRoot = $env:REPOMEOW_DATA_DIR
$originalEncoding = [Console]::OutputEncoding
New-Item -ItemType Directory -Path $root | Out-Null
try {
    $env:REPOMEOW_DATA_DIR = Join-Path $root 'data'
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $project = Join-Path $root 'project'
    New-Item -ItemType Directory -Path $project | Out-Null
    # Use Unicode escapes so the script also works in PS 5.1 without a BOM.
    $name = [string][char]0x4E2D + [char]0x6587
    $captured = & $Exe project add -p $project --name $name
    if ($LASTEXITCODE -ne 0 -or ($captured | ConvertFrom-Json).name -ne $name) { throw 'Variable capture/UTF-8 failed' }
    & $Exe project list > (Join-Path $root 'out') 2> (Join-Path $root 'err')
    if ($LASTEXITCODE -ne 0) { throw 'Direct redirection did not wait' }
    $json = Get-Content -Raw -LiteralPath (Join-Path $root 'out') | ConvertFrom-Json
    if ($json.projects[0].name -ne $name) { throw 'Redirected Unicode mismatch' }
    $pipeline = & $Exe project list | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $pipeline.projects[0].name -ne $name) { throw 'Pipeline failed' }
    try {
        # PS 5.1 treats the expected native stderr as a non-terminating error.
        $ErrorActionPreference = 'Continue'
        & $Exe project add > (Join-Path $root 'out') 2> (Join-Path $root 'err')
        if ($LASTEXITCODE -ne 2 -or (Get-Item -LiteralPath (Join-Path $root 'out') -ErrorAction Stop).Length -ne 0) { throw 'Parse failure/exit code mismatch' }
        if ((Get-Content -Raw -LiteralPath (Join-Path $root 'err') -ErrorAction Stop) -notmatch 'invalid_arguments') { throw 'Missing stderr' }
        & $Exe project get -p (Join-Path $root 'missing') > (Join-Path $root 'out') 2> (Join-Path $root 'err')
        if ($LASTEXITCODE -ne 1) { throw 'Business failure/exit code mismatch' }
    } finally {
        $ErrorActionPreference = 'Stop'
    }
    # Used by test-cli-shells.py to verify failure propagation and cleanup.
    if ($InjectAssertionFailure) { throw 'Injected assertion failure' }
} finally {
    $env:REPOMEOW_DATA_DIR = $originalRoot
    [Console]::OutputEncoding = $originalEncoding
    # Test-owned path only, resolved and checked before recursive cleanup.
    $resolvedRoot = [IO.Path]::GetFullPath($root)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if (!$resolvedRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe cleanup path' }
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
}
# Only successful assertions AND cleanup may publish success/reset the native
# failure code retained by the Actions PowerShell wrapper.
[pscustomobject]@{ passed = $true; shell = $PSVersionTable.PSVersion.ToString(); exe = $Exe } | ConvertTo-Json -Compress
exit 0
