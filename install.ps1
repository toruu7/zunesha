# install.ps1 - install `zou`, the local-first AI coding orchestrator, on Windows.
#
#   irm https://raw.githubusercontent.com/toruu7/zunesha/master/install.ps1 -OutFile "$env:TEMP\install-zou.ps1"
#   powershell -ExecutionPolicy Bypass -File "$env:TEMP\install-zou.ps1"
#
# Or from a checkout:
#
#   .\install.ps1
#
# The script prefers the prebuilt `zou-windows-amd64.zip` (binary + dashboard),
# then falls back to a source build (`git clone` + `cargo build`) when the
# prebuilt is unavailable or `-Source` is passed.
#
# It installs to `%LOCALAPPDATA%\Programs\Zunesha` by default, adds
# `<prefix>\bin` to the *user* PATH idempotently, and records the install in
# `<prefix>\share\zou\install.json` so `zou update` can find newer tags.
#
# The binary is unsigned. SmartScreen may warn on first run; this is expected
# for a small open-source project.

[CmdletBinding()]
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA "Programs\Zunesha"),
    [string]$Repo = "https://github.com/toruu7/zunesha",
    [string]$ReleaseBase = "https://github.com/toruu7/zunesha/releases/latest/download",
    [string]$Version = "",
    [string]$Tarball = "",
    [switch]$Source,
    [string]$CargoProfile = "release",
    [switch]$NoDashboard,
    [switch]$NoModifyPath,
    [switch]$Uninstall,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

function Write-Step([string]$Message) {
    Write-Host "==> $Message"
}

function Write-Warn([string]$Message) {
    Write-Warning $Message
}

function Fail([string]$Message) {
    Write-Error "install.ps1: $Message"
    exit 1
}

function Show-Help {
    @"
install.ps1 - install the zou orchestrator on Windows

Usage: .\install.ps1 [options]

  -Prefix DIR        Install root (default: %LOCALAPPDATA%\Programs\Zunesha).
                     Binary goes to DIR\bin\zou.exe, dashboard to DIR\web\out.
  -Repo URL          Git repository used for source builds.
  -ReleaseBase URL   Base URL for prebuilt zips. The script appends
                     [/VERSION/]zou-windows-amd64.zip.
  -Version TAG       Optional release tag (for example v0.9.0).
  -Tarball URL       Full URL/path of the prebuilt zip (overrides the above).
  -Source            Skip the prebuilt and build from source.
  -CargoProfile NAME Cargo profile for source builds (default: release).
  -NoDashboard       Install the binary only.
  -NoModifyPath      Do not edit the user PATH; print the line instead.
  -Uninstall         Remove the binary, dashboard, record and managed PATH entry.
  -Help              Show this text.

Examples:
  .\install.ps1
  .\install.ps1 -Version v0.9.0
  .\install.ps1 -Source -Repo https://github.com/you/zunesha
  .\install.ps1 -Tarball .\zou-windows-amd64.zip
"@
}

if ($Help) {
    Show-Help
    exit 0
}

# Windows PowerShell 5.1 defaults to old TLS; GitHub requires 1.2+.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
} catch {
    # Newer PowerShell versions negotiate TLS automatically.
}

$Prefix = [System.IO.Path]::GetFullPath($Prefix)
$BinDir = Join-Path $Prefix "bin"
$Bin = Join-Path $BinDir "zou.exe"
$WebDir = Join-Path $Prefix "web\out"
$RecordDir = Join-Path $Prefix "share\zou"
$RecordPath = Join-Path $RecordDir "install.json"
$Asset = "zou-windows-amd64.zip"

function Remove-ManagedPath {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { return }
    $parts = @($userPath -split ";" | Where-Object { $_ -and ($_.TrimEnd([char]'\') -ne $BinDir.TrimEnd([char]'\')) })
    [Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")
}

function Add-ManagedPath {
    if ($NoModifyPath) {
        Write-Step "add this directory to PATH yourself: $BinDir"
        return
    }
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($userPath) { $parts = @($userPath -split ";" | Where-Object { $_ }) }
    $already = $false
    foreach ($part in $parts) {
        if ($part.TrimEnd([char]'\') -eq $BinDir.TrimEnd([char]'\')) { $already = $true; break }
    }
    if (-not $already) {
        $parts += $BinDir
        [Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")
        Write-Step "added $BinDir to the user PATH"
    }
    if (($env:Path -split ";") -notcontains $BinDir) {
        $env:Path = "$BinDir;$env:Path"
    }
    Write-Step "open a new terminal (or refresh PATH) before running zou"
}

function Resolve-PrebuiltUrl {
    if ($Tarball) {
        if (Test-Path $Tarball) { return (Resolve-Path $Tarball).Path }
        return $Tarball
    }
    $base = $ReleaseBase.TrimEnd("/")
    if ($Version) {
        if ($base -match "/releases/latest/download$") {
            $base = $base -replace "/releases/latest/download$", "/releases/download"
        }
        return "$base/$Version/$Asset"
    }
    return "$base/$Asset"
}

function Get-Archive([string]$Url) {
    $temp = Join-Path $env:TEMP ("zou-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $temp | Out-Null
    $zip = Join-Path $temp "zou.zip"
    Write-Step "downloading $Url"
    if (Test-Path $Url) {
        Copy-Item -Path $Url -Destination $zip -Force
    } else {
        Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing
    }
    $expanded = Join-Path $temp "expanded"
    Expand-Archive -Path $zip -DestinationPath $expanded -Force
    return @{ Root = $temp; Expanded = $expanded }
}

function Install-Prebuilt {
    $url = Resolve-PrebuiltUrl
    $archive = Get-Archive $url
    try {
        $exe = Get-ChildItem -Path $archive.Expanded -Filter "zou.exe" -File -Recurse |
            Select-Object -First 1
        if (-not $exe) { throw "the archive did not contain zou.exe" }
        New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
        Copy-Item -Path $exe.FullName -Destination $Bin -Force
        try { Unblock-File -Path $Bin } catch { }

        if (-not $NoDashboard) {
            $out = Get-ChildItem -Path $archive.Expanded -Directory -Recurse |
                Where-Object { $_.Name -eq "out" -and (Test-Path (Join-Path $_.FullName "index.html")) } |
                Select-Object -First 1
            if ($out) {
                if (Test-Path $WebDir) { Remove-Item -Recurse -Force $WebDir }
                New-Item -ItemType Directory -Force -Path $WebDir | Out-Null
                Copy-Item -Path (Join-Path $out.FullName "*") -Destination $WebDir -Recurse -Force
                Write-Step "installed dashboard to $WebDir"
            } else {
                Write-Warn "the archive had no web/out dashboard; run with a dashboard-enabled release or build from source"
            }
        }
    } finally {
        Remove-Item -Recurse -Force $archive.Root -ErrorAction SilentlyContinue
    }
}

function Install-Source {
    foreach ($tool in @("git", "cargo")) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            Fail "source builds need $tool on PATH (or use the prebuilt release)"
        }
    }
    $temp = Join-Path $env:TEMP ("zou-source-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $temp | Out-Null
    $checkout = Join-Path $temp "src"
    try {
        $sourceLabel = $Repo
        if ($Version) { $sourceLabel = "$Repo at $Version" }
        Write-Step "cloning $sourceLabel"
        $cloneArgs = @("clone", "--quiet")
        if (-not (Test-Path $Repo)) { $cloneArgs += "--depth"; $cloneArgs += "1" }
        if ($Version) { $cloneArgs += @("--branch", $Version) }
        $cloneArgs += @($Repo, $checkout)
        & git @cloneArgs
        if ($LASTEXITCODE -ne 0) { throw "git clone failed" }

        Write-Step "building the binary (profile: $CargoProfile)"
        $cargoArgs = @("build", "--manifest-path", (Join-Path $checkout "Cargo.toml"), "-p", "zunesha-cli")
        if ($CargoProfile -ne "dev") { $cargoArgs += "--$CargoProfile" }
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

        $built = Join-Path $checkout "target\$CargoProfile\zou.exe"
        if (-not (Test-Path $built)) { throw "cargo did not produce $built" }
        New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
        Copy-Item -Path $built -Destination $Bin -Force

        if (-not $NoDashboard) {
            if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
                Write-Warn "npm is not on PATH; skipping the dashboard (install Node.js 22+ to build it)"
            } else {
                Write-Step "building the dashboard"
                Push-Location (Join-Path $checkout "web")
                try {
                    & npm ci
                    if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }
                    & npm run build
                    if ($LASTEXITCODE -ne 0) { throw "npm run build failed" }
                } finally {
                    Pop-Location
                }
                $out = Join-Path $checkout "web\out"
                if (Test-Path $out) {
                    if (Test-Path $WebDir) { Remove-Item -Recurse -Force $WebDir }
                    New-Item -ItemType Directory -Force -Path $WebDir | Out-Null
                    Copy-Item -Path (Join-Path $out "*") -Destination $WebDir -Recurse -Force
                    Write-Step "installed dashboard to $WebDir"
                }
            }
        }
    } finally {
        Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
    }
}

function Write-InstallRecord {
    $installedVersion = ""
    try {
        $output = & $Bin --version 2>$null
        if ($output) { $installedVersion = (($output -split "\s+") | Select-Object -Last 1) }
    } catch { }

    $remote = $Repo
    if (Test-Path $Repo) {
        $origin = & git -C $Repo remote get-url origin 2>$null
        if ($LASTEXITCODE -eq 0 -and $origin) { $remote = $origin.Trim() }
    }
    $releaseBase = $ReleaseBase.TrimEnd("/")
    if ($Tarball) { $releaseBase = $null }
    elseif ($releaseBase -match "/releases/latest/download$") {
        $releaseBase = $releaseBase -replace "/releases/latest/download$", "/releases/download"
    }

    New-Item -ItemType Directory -Force -Path $RecordDir | Out-Null
    $record = [ordered]@{
        version      = $installedVersion
        tag          = $Version
        prefix       = $Prefix
        repo         = $Repo
        remote       = $remote
        release_base = $releaseBase
        installed_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    }
    $json = $record | ConvertTo-Json
    [System.IO.File]::WriteAllText($RecordPath, $json, [System.Text.UTF8Encoding]::new($false))
}

if ($Uninstall) {
    Write-Step "removing $Prefix"
    if (Test-Path $Bin) { Remove-Item -Force $Bin }
    if (Test-Path (Join-Path $Prefix "web\out")) { Remove-Item -Recurse -Force (Join-Path $Prefix "web\out") }
    if (Test-Path $RecordDir) { Remove-Item -Recurse -Force $RecordDir }
    Remove-ManagedPath
    Write-Step "uninstalled; open a new terminal to refresh PATH"
    exit 0
}

try {
    if ($Source) {
        Install-Source
    } else {
        try {
            Install-Prebuilt
        } catch {
            Write-Warn "prebuilt install failed: $($_.Exception.Message)"
            Write-Step "falling back to a source build"
            Install-Source
        }
    }

    if (-not (Test-Path $Bin)) { Fail "installation did not produce $Bin" }
    Add-ManagedPath
    Write-InstallRecord
    $versionText = (& $Bin --version) -join " "
    Write-Step "installed $versionText"
    Write-Step "binary: $Bin"
    Write-Step "dashboard: $WebDir"
    Write-Host ""
    Write-Host "Run:"
    Write-Host "  zou model"
    Write-Host "  zou init"
    Write-Host "  zou start"
    Write-Host ""
    Write-Host "If this terminal was opened before the install, open a new one so PATH picks up $BinDir."
} catch {
    Fail $_.Exception.Message
}
