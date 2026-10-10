# Installs a jackdaw release for the current user, or removes it.
#
#   irm https://github.com/jbuehler23/jackdaw/releases/latest/download/install.ps1 | iex
#
# With options:
#
#   & ([scriptblock]::Create((irm https://github.com/jbuehler23/jackdaw/releases/latest/download/install.ps1))) -Version 0.19.0-rc.9
[CmdletBinding()]
param(
    [string]$Version = $env:JACKDAW_VERSION,
    [string]$Prefix = $env:JACKDAW_INSTALL_DIR,
    [switch]$NoModifyPath,
    [switch]$Yes,
    [switch]$Uninstall,
    [string]$BaseUrl = $env:JACKDAW_DOWNLOAD_BASE,
    [switch]$Help
)

function Write-Step([string]$Message) {
    Write-Host "jackdaw-install: $Message"
}

function Test-Bundle([string]$Dir) {
    $item = Get-Item -LiteralPath $Dir -Force -ErrorAction SilentlyContinue
    return $item -and $item.PSIsContainer -and
        -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -and
        (Test-Path -LiteralPath (Join-Path $Dir 'jd.exe')) -and
        (Test-Path -LiteralPath (Join-Path $Dir 'sdk\manifest.txt'))
}

function Get-Download([string]$Url, [string]$OutFile) {
    for ($attempt = 1; ; $attempt++) {
        try {
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
            return
        } catch {
            if ($attempt -ge 3) { throw "could not download ${Url}: $($_.Exception.Message)" }
            Start-Sleep -Seconds 2
        }
    }
}

# The latest release is read from where /releases/latest redirects, so the
# version directory is named before anything is downloaded and no API call
# (and no rate limit) is involved.
function Get-LatestVersion([string]$Repo) {
    $request = [Net.HttpWebRequest]::Create("https://github.com/$Repo/releases/latest")
    $request.AllowAutoRedirect = $false
    $request.Method = 'HEAD'
    $request.UserAgent = 'jackdaw-install'
    try {
        $response = $request.GetResponse()
    } catch [Net.WebException] {
        $response = $_.Exception.Response
    }
    if (-not $response) { throw 'could not reach github.com to find the latest release' }
    $location = $response.Headers['Location']
    $response.Close()
    if ($location -notmatch '/tag/v?([^/]+)$') { throw 'could not find the latest release; pass -Version' }
    return $Matches[1]
}

function Get-PathEntries([string]$Value) {
    return @($Value -split ';' | Where-Object { $_ -ne '' })
}

function Test-SamePath([string]$Entry, [string]$Dir) {
    $expanded = [Environment]::ExpandEnvironmentVariables($Entry).TrimEnd('\')
    return $expanded -ieq $Dir.TrimEnd('\')
}

# Reads and writes the raw registry value so entries such as %USERPROFILE%\bin
# stay unexpanded, then broadcasts the change so new terminals see it.
function Update-UserPath([string]$Dir, [bool]$Add) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $raw = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        if ($key.GetValueNames() -contains 'Path') { $kind = $key.GetValueKind('Path') }
        $entries = Get-PathEntries $raw
        $present = @($entries | Where-Object { Test-SamePath $_ $Dir }).Count -gt 0
        if ($Add -and -not $present) {
            $key.SetValue('Path', ((@($Dir) + $entries) -join ';'), $kind)
        } elseif (-not $Add -and $present) {
            $kept = @($entries | Where-Object { -not (Test-SamePath $_ $Dir) })
            if ($kept.Count -gt 0) { $key.SetValue('Path', ($kept -join ';'), $kind) } else { $key.DeleteValue('Path', $false) }
        } else {
            return $false
        }
    } finally {
        $key.Close()
    }
    $dummy = 'jackdaw-install-' + [guid]::NewGuid().ToString()
    [Environment]::SetEnvironmentVariable($dummy, '1', 'User')
    [Environment]::SetEnvironmentVariable($dummy, [NullString]::Value, 'User')
    return $true
}

function Remove-Junction([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        # Deleting the link itself; a recursive Remove-Item on a junction can
        # empty its target on Windows PowerShell.
        [IO.Directory]::Delete($Path)
    }
}

function Invoke-JackdawInstall {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    $repo = 'jbuehler23/jackdaw'
    if (-not $Version) { $Version = $env:JACKDAW_VERSION }
    if (-not $Prefix) { $Prefix = $env:JACKDAW_INSTALL_DIR }
    if (-not $BaseUrl) { $BaseUrl = $env:JACKDAW_DOWNLOAD_BASE }
    $triple = 'x86_64-pc-windows-msvc'

    if ($Help) {
        Write-Host @'
Install jackdaw from a GitHub release for the current user.

  -Version <v>      Install release v<v>, e.g. 0.19.0-rc.9 (default: latest)
  -Prefix <dir>     Where versions are kept (default: %LOCALAPPDATA%\jackdaw\install)
  -NoModifyPath     Do not add the install to the user PATH
  -Yes              Accepted for parity with install.sh; this script never asks
  -Uninstall        Remove what this script installed

Environment: JACKDAW_VERSION, JACKDAW_INSTALL_DIR, JACKDAW_NO_MODIFY_PATH=1.
'@
        return
    }
    if ($PSVersionTable.PSVersion.Major -lt 5) { throw 'PowerShell 5.1 or newer is required' }
    if (-not $env:LOCALAPPDATA -and -not $Prefix) { throw 'LOCALAPPDATA is not set; pass -Prefix' }

    $root = $Prefix
    if (-not $root) { $root = Join-Path $env:LOCALAPPDATA 'jackdaw\install' }
    $root = [IO.Path]::GetFullPath($root).TrimEnd('\')
    $current = Join-Path $root 'current'
    $previousFile = Join-Path $root 'previous'
    $modifyPath = -not $NoModifyPath -and (-not $env:JACKDAW_NO_MODIFY_PATH -or $env:JACKDAW_NO_MODIFY_PATH -eq '0')

    if ($Uninstall) {
        if (Update-UserPath $current $false) { Write-Step "removed $current from the user PATH" }
        $env:Path = (@(Get-PathEntries $env:Path | Where-Object { -not (Test-SamePath $_ $current) }) -join ';')
        if (Test-Path -LiteralPath $root) {
            Remove-Junction $current
            Remove-Item -LiteralPath $previousFile -Force -ErrorAction SilentlyContinue
            Get-ChildItem -LiteralPath $root -Directory -Force | Where-Object { Test-Bundle $_.FullName } |
                ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force }
            if (@(Get-ChildItem -LiteralPath $root -Force).Count -eq 0) {
                Remove-Item -LiteralPath $root -Force
            } else {
                Write-Step "left $root in place: it holds files this script did not install"
            }
        }
        Write-Step 'jackdaw is uninstalled. Projects, settings, extensions and any SDK cache are kept.'
        return
    }

    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') {
        throw "no prebuilt jackdaw for Windows $arch. Install with 'cargo install jackdaw --locked' instead; see https://jbuehler23.github.io/jackdaw/getting-started/installation.html"
    }

    if ($PSVersionTable.PSEdition -ne 'Core') {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
    if (-not $Version) {
        if ($BaseUrl) { throw '-BaseUrl needs -Version' }
        $Version = Get-LatestVersion $repo
    }
    $Version = $Version -replace '^v', ''
    if ($Version -notmatch '^[0-9A-Za-z][0-9A-Za-z.+-]*$') { throw "'$Version' is not a release version" }
    $base = $BaseUrl
    if (-not $base) { $base = "https://github.com/$repo/releases/download/v$Version" }
    $base = $base.TrimEnd('/')

    $asset = "jackdaw-$triple.zip"
    $dest = Join-Path $root $Version
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('jackdaw-install-' + [guid]::NewGuid().ToString())
    $stage = Join-Path $root ('.staging-' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Step "downloading $base/$asset"
        Get-Download "$base/$asset.sha256" (Join-Path $tmp "$asset.sha256")
        Get-Download "$base/$asset" (Join-Path $tmp $asset)

        $text = Get-Content -LiteralPath (Join-Path $tmp "$asset.sha256") -Raw
        if ($text -notmatch '([0-9a-fA-F]{64})') { throw "$asset.sha256 holds no SHA-256 digest" }
        $expected = $Matches[1].ToLowerInvariant()
        $actual = (Get-FileHash -LiteralPath (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "checksum mismatch for ${asset}: expected $expected, got $actual. Nothing was installed."
        }

        New-Item -ItemType Directory -Path $stage -Force | Out-Null
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $tmp $asset), $stage)
        $bundle = Join-Path $stage "jackdaw-$triple"
        if (-not (Test-Bundle $bundle)) { throw "$asset does not hold a jackdaw-$triple folder with jd.exe and sdk\manifest.txt" }

        $wasCurrent = $null
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($item -and $item.Target) { $wasCurrent = Split-Path -Leaf (@($item.Target)[0]) }
        if (Test-Path -LiteralPath $dest) {
            try {
                Move-Item -LiteralPath $dest -Destination (Join-Path $stage 'replaced')
            } catch {
                throw "could not replace $dest, probably because jackdaw is running from it. Close jackdaw and jd, then run this again."
            }
        }
        Move-Item -LiteralPath $bundle -Destination $dest
        Remove-Junction $current
        New-Item -ItemType Junction -Path $current -Target $dest | Out-Null
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }

    # The version that was current before an upgrade is kept for rollback;
    # older ones are removed.
    if ($wasCurrent -and $wasCurrent -ne $Version) { Set-Content -LiteralPath $previousFile -Value $wasCurrent }
    $previous = $null
    if (Test-Path -LiteralPath $previousFile) { $previous = (Get-Content -LiteralPath $previousFile -Raw).Trim() }
    if ($previous -eq $Version) {
        Remove-Item -LiteralPath $previousFile -Force
        $previous = $null
    }
    Get-ChildItem -LiteralPath $root -Directory -Force |
        Where-Object { $_.Name -ne $Version -and $_.Name -ne $previous -and (Test-Bundle $_.FullName) } |
        ForEach-Object {
            $old = $_.FullName
            try {
                Remove-Item -LiteralPath $old -Recurse -Force
            } catch {
                Write-Step "could not remove $old, which may be in use; delete it later"
            }
        }

    Write-Step "installed jackdaw $Version to $dest"

    if ($modifyPath) {
        if (Update-UserPath $current $true) { Write-Step "added $current to the user PATH; new terminals will see it" }
        if (-not (@(Get-PathEntries $env:Path | Where-Object { Test-SamePath $_ $current }).Count)) {
            $env:Path = "$current;$env:Path"
        }
        $found = Get-Command jackdaw -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found -and -not (Test-SamePath (Split-Path -Parent $found.Source) $current)) {
            Write-Step "warning: $($found.Source) comes before $current on PATH and will run instead"
        }
    } else {
        Write-Step "add $current to PATH to run jackdaw by name"
    }
    Write-Step "run 'jackdaw' to open the editor, or 'jd doctor' to check your setup"
}

Invoke-JackdawInstall
