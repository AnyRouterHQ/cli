# AnyRouter CLI installer for Windows. Downloads GitHub Release binaries.
#   irm https://anyrouter.dev/cli.ps1 | iex
#   irm https://raw.githubusercontent.com/AnyRouterHQ/cli/main/install.ps1 | iex
#   .\install.ps1 [-Channel stable|beta] [-Version X.Y.Z] [-BinDir DIR]
#
# Env (used when the matching parameter is not given):
#   ANYR_CHANNEL    stable (default) | beta
#   ANYR_VERSION    install this tag (v prefix optional)
#   ANYR_BIN_DIR    install directory (default: %LOCALAPPDATA%\Programs\anyr)
#   ANYR_SETUP_BIN  copy this file instead of downloading (tests)
#   GH_TOKEN / GITHUB_TOKEN  optional; lists releases through the GitHub API
#
# Release lookup matches setup.sh: a pinned version installs that tag; beta is
# the newest prerelease that has the asset; stable is /releases/latest when it
# has the asset, else the newest stable with it, else the newest release with
# it. The download is checked against the release's checksums.txt. No admin
# rights are needed. Works in Windows PowerShell 5.1 and PowerShell 7.
param(
    [string]$Channel = $env:ANYR_CHANNEL,
    [string]$Version = $env:ANYR_VERSION,
    [string]$BinDir = $env:ANYR_BIN_DIR
)

$GitHub = 'https://github.com/AnyRouterHQ/cli'
$GitHubApi = 'https://api.github.com/repos/AnyRouterHQ/cli'
$Asset = 'anyr-windows-x86_64.exe'

function Write-Step([string]$Label, [string]$Text) {
    Write-Host ('  + {0,-11} {1}' -f $Label, $Text)
}

function Write-Note([string]$Text) {
    Write-Host "    $Text" -ForegroundColor DarkGray
}

function Get-Url([string]$Url, [string]$OutFile) {
    $headers = @{ 'User-Agent' = 'anyr-setup' }
    $req = @{ Uri = $Url; UseBasicParsing = $true; Headers = $headers }
    if ($OutFile) { $req.OutFile = $OutFile }
    Invoke-WebRequest @req
}

function Test-Url([string]$Url) {
    try {
        Invoke-WebRequest -Uri $Url -Method Head -UseBasicParsing -Headers @{ 'User-Agent' = 'anyr-setup' } | Out-Null
        return $true
    } catch {
        return $false
    }
}

function Get-TagUrl([string]$Tag) {
    return "$GitHub/releases/download/$Tag/$Asset"
}

# Sort key for v1.2.3 and v1.2.3-beta.4: a release sorts above its prereleases.
function Get-VersionKey([string]$Tag) {
    $parts = $Tag.TrimStart('v').Split('-', 2)
    $nums = @(0, 0, 0)
    $core = $parts[0].Split('.')
    for ($i = 0; $i -lt 3 -and $i -lt $core.Count; $i++) {
        [int]$n = 0
        if ([int]::TryParse($core[$i], [ref]$n)) { $nums[$i] = $n }
    }
    $pre = 0
    if ($parts.Count -gt 1) {
        $m = [regex]::Match($parts[1], '(\d+)$')
        if ($m.Success) { $pre = [long]$m.Groups[1].Value }
        $stable = 0
    } else {
        $stable = 1
    }
    return '{0:D6}.{1:D6}.{2:D6}.{3}.{4:D12}' -f $nums[0], $nums[1], $nums[2], $stable, $pre
}

function Find-TagFromApi([string]$Token) {
    $req = @{
        Uri = "$GitHubApi/releases"
        UseBasicParsing = $true
        Headers = @{
            'User-Agent' = 'anyr-setup'
            'Accept' = 'application/vnd.github+json'
            'Authorization' = "Bearer $Token"
        }
    }
    $releases = Invoke-RestMethod @req
    $usable = @($releases | Where-Object { -not $_.draft -and ($_.assets | Where-Object { $_.name -eq $Asset }) })
    if ($Channel -eq 'beta') {
        $pick = $usable | Where-Object { $_.prerelease } | Select-Object -First 1
    } else {
        $pick = $usable | Where-Object { -not $_.prerelease } | Select-Object -First 1
        if (-not $pick) { $pick = $usable | Select-Object -First 1 }
    }
    if ($pick) { return $pick.tag_name }
    return $null
}

function Find-TagFromHtml {
    $html = (Get-Url "$GitHub/releases").Content
    $tags = [regex]::Matches($html, 'releases/tag/(v[0-9][^"<>\s#?/]*)') |
        ForEach-Object { $_.Groups[1].Value } |
        Select-Object -Unique |
        Sort-Object -Property @{ Expression = { Get-VersionKey $_ } } -Descending
    if (-not $tags) { throw "no GitHub release tags found at $GitHub/releases" }
    if ($Channel -eq 'beta') {
        $order = @($tags | Where-Object { $_ -like '*-*' })
    } else {
        $order = @($tags | Where-Object { $_ -notlike '*-*' }) + @($tags | Where-Object { $_ -like '*-*' })
    }
    foreach ($tag in $order) {
        if (Test-Url (Get-TagUrl $tag)) { return $tag }
    }
    return $null
}

function Resolve-DownloadUrl {
    if ($Version) { return Get-TagUrl ('v' + $Version.TrimStart('v')) }
    $token = if ($env:GH_TOKEN) { $env:GH_TOKEN } else { $env:GITHUB_TOKEN }
    if ($token) {
        try {
            $tag = Find-TagFromApi $token
            if ($tag) { return Get-TagUrl $tag }
        } catch {
            Write-Note "authenticated GitHub API failed; listing $GitHub/releases instead"
        }
    }
    if ($Channel -eq 'stable') {
        $latest = "$GitHub/releases/latest/download/$Asset"
        if (Test-Url $latest) { return $latest }
    }
    $tag = Find-TagFromHtml
    if (-not $tag) { throw "no $Channel GitHub release has $Asset" }
    return Get-TagUrl $tag
}

# Refuse a download that checksums.txt does not vouch for.
function Confirm-Checksum([string]$Url, [string]$File, [string]$Work) {
    $sums = Join-Path $Work 'checksums.txt'
    $sumsUrl = $Url.Substring(0, $Url.LastIndexOf('/')) + '/checksums.txt'
    try {
        Get-Url $sumsUrl $sums | Out-Null
    } catch {
        throw "could not fetch $sumsUrl; refusing to install an unverified binary"
    }
    $expected = $null
    foreach ($line in Get-Content -LiteralPath $sums) {
        $f = $line.Trim() -split '\s+', 2
        if ($f.Count -eq 2 -and $f[1].TrimStart('*') -eq $Asset) { $expected = $f[0].ToLowerInvariant() }
    }
    if (-not $expected) { throw "checksums.txt has no entry for $Asset; refusing to install" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $File).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "checksum mismatch for $Asset (expected $expected, got $actual); refusing to install"
    }
    Write-Step 'Verified' ('sha256 {0}...{1}' -f $actual.Substring(0, 8), $actual.Substring(56))
}

# Temp file beside the target, then a rename. A running anyr.exe cannot be
# overwritten, but it can be renamed out of the way.
function Install-Binary([string]$Source, [string]$Dir) {
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $dest = Join-Path $Dir 'anyr.exe'
    $staged = Join-Path $Dir ".anyr.exe.new.$PID"
    $old = Join-Path $Dir '.anyr.exe.old'
    Copy-Item -LiteralPath $Source -Destination $staged -Force
    if (Test-Path -LiteralPath $dest) {
        Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $dest -Destination $old -Force
    }
    try {
        Move-Item -LiteralPath $staged -Destination $dest
    } catch {
        if (Test-Path -LiteralPath $old) { Move-Item -LiteralPath $old -Destination $dest -Force }
        Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
        throw "could not install $dest`: $_"
    }
    Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
    Write-Step 'Installed' $dest
    return $dest
}

function Add-ToUserPath([string]$Dir) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ } | ForEach-Object { $_.TrimEnd('\') })
    if ($entries -contains $Dir.TrimEnd('\')) { return }
    $next = if ($userPath) { $userPath.TrimEnd(';') + ';' + $Dir } else { $Dir }
    [Environment]::SetEnvironmentVariable('Path', $next, 'User')
    $env:Path = (@($env:Path, $Dir) | Where-Object { $_ }) -join ';'
    Write-Step 'PATH' "added $Dir to your user PATH; open a new terminal to use anyr"
}

function Install-Anyr {
    if (-not $Channel) { $Channel = 'stable' }
    $Channel = $Channel.ToLowerInvariant()
    if ($Channel -ne 'stable' -and $Channel -ne 'beta') {
        throw "unknown channel `"$Channel`" (use stable or beta)"
    }
    if (-not $BinDir) { $BinDir = Join-Path $env:LOCALAPPDATA 'Programs\anyr' }

    Write-Host "  anyr  AnyRouter CLI installer`n"
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { Write-Step 'Detected' "windows/x86_64 - $Channel channel" }
        'ARM64' {
            Write-Step 'Detected' "windows/arm64 - $Channel channel"
            Write-Note 'No arm64 Windows build yet: installing the x64 build, which runs under emulation.'
        }
        default { throw "unsupported Windows architecture: $arch (need x64 or arm64)" }
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ("anyr-setup-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    try {
        if ($env:ANYR_SETUP_BIN) {
            if (-not (Test-Path -LiteralPath $env:ANYR_SETUP_BIN -PathType Leaf)) {
                throw "ANYR_SETUP_BIN is not a file: $env:ANYR_SETUP_BIN"
            }
            Write-Note "ANYR_SETUP_BIN=$env:ANYR_SETUP_BIN (local, no download)"
            $source = $env:ANYR_SETUP_BIN
        } else {
            $url = Resolve-DownloadUrl
            $m = [regex]::Match($url, '/releases/download/(v[^/]+)/')
            if ($m.Success) { Write-Step 'Release' $m.Groups[1].Value }
            $source = Join-Path $work $Asset
            try {
                Get-Url $url $source | Out-Null
            } catch {
                throw "download failed: $url ($($_.Exception.Message))"
            }
            $bytes = [IO.File]::ReadAllBytes($source)
            if ($bytes.Length -lt 2 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
                throw "download did not look like a Windows binary (got HTML?): $url"
            }
            Write-Step 'Downloaded' ('{0} ({1:N1} MB)' -f $Asset, ($bytes.Length / 1MB))
            Confirm-Checksum $url $source $work
        }
        $dest = Install-Binary $source $BinDir
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    $saved = $env:ANYR_NO_UPDATE
    $env:ANYR_NO_UPDATE = '1'
    try {
        $out = & $dest --version | Out-String
        $code = $LASTEXITCODE
    } finally {
        $env:ANYR_NO_UPDATE = $saved
    }
    if ($code -ne 0) { throw "$dest --version failed (exit $code): $out" }
    Write-Step 'Version' $out.Trim()
    Add-ToUserPath $BinDir
    Write-Host "`n  Next"
    Write-Host '    anyr auth login   sign in'
    Write-Host '    anyr claude       launch Claude Code through AnyRouter'
}

# Run through `irm | iex`, so restore the caller's session settings and fail
# with `throw`: an `exit` would close the user's terminal.
$savedProgress = $ProgressPreference
$savedErrors = $ErrorActionPreference
$savedTls = [Net.ServicePointManager]::SecurityProtocol
try {
    $ProgressPreference = 'SilentlyContinue'
    $ErrorActionPreference = 'Stop'
    [Net.ServicePointManager]::SecurityProtocol = $savedTls -bor [Net.SecurityProtocolType]::Tls12
    Install-Anyr
} catch {
    Write-Host "`n  x $($_.Exception.Message)`n" -ForegroundColor Red
    throw
} finally {
    $ProgressPreference = $savedProgress
    $ErrorActionPreference = $savedErrors
    [Net.ServicePointManager]::SecurityProtocol = $savedTls
}
