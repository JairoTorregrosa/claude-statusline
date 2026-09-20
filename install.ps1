<#
.SYNOPSIS
    Installer for claude-statusline on Windows. Safe to re-run.

.DESCRIPTION
    Builds the binary, installs it to %USERPROFILE%\.local\bin, and points
    %USERPROFILE%\.claude\settings.json at it (with a backup of the previous
    value). The PowerShell counterpart of install.sh: same steps, same
    guarantees, same rollback information.

    Works under Windows PowerShell 5.1 and PowerShell 7. Run it as a file
    from a clone of the repository:

        powershell -ExecutionPolicy Bypass -File .\install.ps1
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# PowerShell 7.4+ turns a native command's non-zero exit into a terminating
# error before we can read $LASTEXITCODE. Keep both generations on the same
# path: we check exit codes ourselves and fail with our own message.
if ($PSVersionTable.PSVersion.Major -ge 7) { $PSNativeCommandUseErrorActionPreference = $false }

function Say { param([string]$Message) Write-Host "> $Message" }
function Die { param([string]$Message) Write-Error "error: $Message"; exit 1 }

# The BOM-less write below needs .NET calls that Constrained Language Mode
# (AppLocker / WDAC policies) forbids. Say so up front instead of failing
# halfway with a security exception.
if ($ExecutionContext.SessionState.LanguageMode -ne 'FullLanguage') {
    Die "this script needs PowerShell FullLanguage mode (current: $($ExecutionContext.SessionState.LanguageMode)). Build with 'cargo build --release' and add the statusLine entry to settings.json by hand; see README.md."
}

# $PSScriptRoot is empty when the content is piped into Invoke-Expression.
# The build and the copy below are relative to the repository, so refuse
# rather than run cargo in whatever directory the user happens to be in.
if (-not $PSScriptRoot) {
    Die "run this script as a file from a clone of the repository: powershell -ExecutionPolicy Bypass -File .\install.ps1"
}

# Locate Git Bash the way Claude Code does, because Claude Code runs the
# statusLine command through Git Bash when Git for Windows is present and
# through PowerShell when it is absent (docs: "Set up on Windows"). The two
# shells quote a path differently, so when the path needs quoting we must
# know which shell will read it.
function Find-GitBash {
    param($Settings)
    $candidates = New-Object System.Collections.Generic.List[string]
    if ($env:CLAUDE_CODE_GIT_BASH_PATH) { $candidates.Add($env:CLAUDE_CODE_GIT_BASH_PATH) }
    # The documented place to pin it is the `env` block of settings.json.
    $envBlock = $Settings.PSObject.Properties['env']
    if ($envBlock -and $envBlock.Value -is [System.Management.Automation.PSCustomObject] -and
        $envBlock.Value.PSObject.Properties['CLAUDE_CODE_GIT_BASH_PATH']) {
        $candidates.Add([string]$envBlock.Value.CLAUDE_CODE_GIT_BASH_PATH)
    }
    # `git --exec-path` always points inside the Git for Windows tree
    # regardless of which git.exe won on PATH; bash.exe lives a few levels up
    # (three for the stock layout; six leaves room for deeper ones). A
    # minimal Git (MinGit) has no bash.exe and correctly yields nothing.
    #
    # Under Windows PowerShell 5.1 with $ErrorActionPreference = 'Stop', a
    # redirected stderr line from a native command becomes a terminating
    # error. A git that merely prints a warning must not abort the install,
    # so this probe runs with the preference relaxed and swallows failures.
    $execPath = $null
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $lines = @(& git --exec-path 2>$null | Where-Object { $_ -is [string] -and $_.Trim() })
        if ($LASTEXITCODE -eq 0 -and $lines.Count -gt 0) { $execPath = $lines[0].Trim() }
    } catch {
        $execPath = $null
    } finally {
        $ErrorActionPreference = $savedPreference
    }
    if ($execPath) {
        $dir = [string]$execPath
        for ($i = 0; $i -lt 6 -and $dir; $i++) {
            $candidates.Add((Join-Path $dir 'bin\bash.exe'))
            $candidates.Add((Join-Path $dir 'usr\bin\bash.exe'))
            $dir = Split-Path -Parent $dir
        }
    }
    $roots = @($env:ProgramFiles, ${env:ProgramFiles(x86)})
    if ($env:LOCALAPPDATA) { $roots += Join-Path $env:LOCALAPPDATA 'Programs' }
    foreach ($root in $roots) {
        if ($root) { $candidates.Add((Join-Path $root 'Git\bin\bash.exe')) }
    }
    foreach ($c in $candidates) {
        if ($c -and (Test-Path -LiteralPath $c -PathType Leaf)) { return $c }
    }
    return $null
}

foreach ($tool in 'cargo', 'git') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        if ($tool -eq 'cargo') { Die "Rust is required. Install it from https://rustup.rs" }
        Die "$tool is required"
    }
}

# Claude Code keeps its settings under the profile directory, which is what
# PowerShell's $HOME resolves to. The binary itself reads HOME before
# USERPROFILE (src/cache.rs), so a HOME that points elsewhere makes the
# binary look for .claude in a place Claude Code never writes. Warn; do not
# guess.
if ($env:HOME) {
    $homeResolved = Resolve-Path -LiteralPath $env:HOME -ErrorAction SilentlyContinue
    $profileResolved = Resolve-Path -LiteralPath $HOME -ErrorAction SilentlyContinue
    if (-not $homeResolved) {
        Say "warning: HOME is set to '$env:HOME', which does not exist. The binary reads ~/.claude from HOME; Claude Code reads from your profile '$HOME'. Leave HOME unset, or set it to the profile."
    } elseif ($profileResolved -and ($homeResolved.Path -ne $profileResolved.Path)) {
        Say "warning: HOME is set to '$env:HOME' but your profile is '$HOME'. The binary will read ~/.claude from HOME; Claude Code reads from the profile. Leave HOME unset, or set it to the profile."
    }
}

Set-Location -LiteralPath $PSScriptRoot

Say 'building (release)'
cargo build --release
if ($LASTEXITCODE -ne 0) { Die 'cargo build failed' }

$binDir = Join-Path $HOME '.local\bin'
$bin = Join-Path $binDir 'claude-statusline.exe'
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

# Windows refuses to overwrite an executable that is running — and the
# status line runs on every render — but it allows renaming one. Move the
# old binary aside, copy the new one in, then remove the old one if nothing
# still holds it. install(1) gets the same effect for free on POSIX.
$built = Join-Path $PSScriptRoot 'target\release\claude-statusline.exe'
$previousBin = "$bin.previous"
try {
    if (Test-Path -LiteralPath $bin) {
        if (Test-Path -LiteralPath $previousBin) { Remove-Item -LiteralPath $previousBin -Force -ErrorAction SilentlyContinue }
        Move-Item -LiteralPath $bin -Destination $previousBin -Force
    }
    Copy-Item -LiteralPath $built -Destination $bin -Force
} catch {
    $reason = $_.Exception.Message
    # Put the old binary back so a failed upgrade never leaves the user
    # with no status line at all — and if even that fails, say where it is.
    if ((Test-Path -LiteralPath $previousBin) -and -not (Test-Path -LiteralPath $bin)) {
        Move-Item -LiteralPath $previousBin -Destination $bin -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath $bin)) {
            Die "could not install $bin ($reason), and could not move the previous binary back. It is intact at $previousBin; move it to $bin by hand."
        }
        Say "previous binary restored"
    }
    Die "could not install $bin ($reason). If a Claude Code session is rendering the status line, wait a moment and re-run."
}
if (Test-Path -LiteralPath $previousBin) { Remove-Item -LiteralPath $previousBin -Force -ErrorAction SilentlyContinue }
Say "installed $bin"

Say 'verifying binary'
$null = '{}' | & $bin
if ($LASTEXITCODE -ne 0) { Die 'binary failed the smoke test' }

$settingsPath = Join-Path $HOME '.claude\settings.json'
$settings = [pscustomobject]@{}

if (Test-Path -LiteralPath $settingsPath) {
    $raw = Get-Content -LiteralPath $settingsPath -Raw -Encoding UTF8
    try {
        $settings = $raw | ConvertFrom-Json
    } catch {
        Die "$settingsPath is not valid JSON ($($_.Exception.Message)). Fix it first; refusing to overwrite."
    }
    # ConvertFrom-Json returns nothing for an empty file and an array for a
    # top-level array; neither throws. Both would be silently mangled by the
    # merge below, so refuse them the same way install.sh refuses bad JSON.
    if ($null -eq $settings) {
        Die "$settingsPath is empty. Fix it first; refusing to overwrite."
    }
    if ($settings -isnot [System.Management.Automation.PSCustomObject]) {
        Die "$settingsPath does not hold a JSON object at the top level. Fix it first; refusing to overwrite."
    }
    $backup = [IO.Path]::ChangeExtension($settingsPath, '.json.bak')
    Copy-Item -LiteralPath $settingsPath -Destination $backup -Force
    Say "backup written to $backup"

    # Same rule as install.sh's `if previous:` — nothing to roll back to when
    # the value is null, false, empty, or an object with no members.
    $existing = $settings.PSObject.Properties['statusLine']
    if ($existing -and $null -ne $existing.Value -and $existing.Value -ne $false -and "$($existing.Value)" -ne '' -and
        -not ($existing.Value -is [System.Management.Automation.PSCustomObject] -and @($existing.Value.PSObject.Properties).Count -eq 0)) {
        $previous = $existing.Value | ConvertTo-Json -Depth 100 -Compress
        Say "previous statusLine (rollback value): $previous"
    }
}

# Forward slashes, deliberately: in Git Bash a backslash is an escape
# character and the path reaches the shell with its separators removed.
$binForSettings = $bin -replace '\\', '/'

# A bare path is a valid command in Git Bash, PowerShell and cmd alike — but
# only while it contains nothing the shell interprets. Windows user names may
# contain spaces, quotes, `&`, `(`, `$` and more, and every one of those
# breaks an unquoted path silently. Single quotes are literal in both shells;
# only the quote character itself is escaped, and each shell spells that
# differently. `&` (the call operator) is how PowerShell runs a quoted path,
# and is a syntax error in Bash — hence the detection above.
if ($binForSettings -match '^[A-Za-z0-9_./:\-]+$') {
    $command = $binForSettings
} else {
    $forBash = "'" + ($binForSettings -replace "'", "'\''") + "'"
    $forPwsh = "& '" + ($binForSettings -replace "'", "''") + "'"
    if (Find-GitBash -Settings $settings) {
        $command = $forBash
        Say 'path needs quoting; quoted for Git Bash, which Claude Code uses when Git for Windows is installed'
        Say "  if Claude Code runs it through PowerShell instead, set command to: $forPwsh"
    } else {
        $command = $forPwsh
        Say 'path needs quoting and Git Bash was not found; quoted for PowerShell'
        Say "  if you install Git for Windows later, set command to: $forBash"
    }
}

$statusLine = [pscustomobject]@{
    type    = 'command'
    command = $command
}
# Assign in place when the key exists so it keeps its position in the file,
# as a dict update does in install.sh. Add-Member -Force would drop and
# re-add it at the end, churning every diff for anyone tracking the file.
if ($settings.PSObject.Properties['statusLine']) {
    $settings.statusLine = $statusLine
} else {
    $settings | Add-Member -NotePropertyName 'statusLine' -NotePropertyValue $statusLine
}

# -Depth 100 is not decoration. ConvertTo-Json defaults to depth 2 and
# silently replaces anything deeper with a placeholder string, which would
# destroy nested keys such as `hooks` in a real settings file.
$json = $settings | ConvertTo-Json -Depth 100

# ConvertTo-Json emits CRLF on Windows PowerShell. install.sh writes LF, and
# a settings file that flips line endings depending on which installer last
# touched it churns every line for anyone tracking it in a dotfiles repo.
$json = $json -replace "`r`n", "`n"

# Windows PowerShell 5.1 writes UTF-8 with a BOM through Set-Content and
# Out-File. A BOM makes strict JSON parsers reject the file, so write the
# bytes ourselves with a BOM-less encoder.
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $settingsPath) | Out-Null
[IO.File]::WriteAllText($settingsPath, $json + "`n", (New-Object Text.UTF8Encoding $false))
Say "$settingsPath updated: statusLine.command = $command"

Say 'done. The statusline appears on the next Claude Code render.'
