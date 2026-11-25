<#
.SYNOPSIS
    Build and package anterm Windows release artifacts into dist/.

.DESCRIPTION
    Builds `anterm-app`, `anterm-server`, and `anterm-updater` separately in release mode
    (preserving Cargo feature isolation and static MSVC CRT linking from .cargo/config.toml),
    verifies no dynamic VCRUNTIME/MSVCP DLL imports exist, assembles a portable ZIP matching
    `anterm-updater`'s exact `PORTABLE_MANAGED_ROOTS` layout with canonical `/` ZIP separators,
    copies the standalone `anterm-server` binary, writes `checksums.txt` and `SHA256SUMS.txt`,
    and runs `anterm-updater.exe verify-portable` as a smoke check.

.PARAMETER SkipBuild
    Skip `cargo build --release` and package existing binaries from target/.

.PARAMETER Target
    Optional Cargo target triple (e.g. `x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc`).
    Defaults to the host Windows architecture.

.PARAMETER OutDir
    Output directory for release artifacts (default: `dist`).

.PARAMETER SkipVerify
    Skip executing `anterm-server` and `anterm-updater verify-portable` smoke checks
    (useful when cross-compiling ARM64 artifacts on an x64 host).

.EXAMPLE
    .\scripts\package-windows.ps1

.EXAMPLE
    .\scripts\package-windows.ps1 -Target aarch64-pc-windows-msvc -SkipVerify
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [string]$Target = "",
    [string]$OutDir = "",
    [switch]$SkipVerify
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RootDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $RootDir "dist"
} elseif (-not [System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir = Join-Path $RootDir $OutDir
}

# Read shared workspace version from root Cargo.toml.
$CargoTomlPath = Join-Path $RootDir "Cargo.toml"
$VersionMatch = Select-String -Path $CargoTomlPath -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $VersionMatch) {
    throw "Failed to read workspace version from $CargoTomlPath"
}
$Version = $VersionMatch.Matches[0].Groups[1].Value

# Resolve architecture labels matching:
# - desktop portable ZIP: `anterm-<version>-windows-<x86_64|arm64>.zip`
# - standalone server:    `anterm-server-windows-<x86_64|aarch64>.exe`
# - bundled ConPTY dir:   `assets/windows/conpty/<x64|arm64>`
function Resolve-ArchInfo {
    param([string]$Triple)

    if (-not [string]::IsNullOrWhiteSpace($Triple)) {
        switch -Wildcard ($Triple) {
            "x86_64-*"  { return @{ Desktop = "x86_64"; Server = "x86_64";  Conpty = "x64"   } }
            "aarch64-*" { return @{ Desktop = "arm64";  Server = "aarch64"; Conpty = "arm64" } }
            default     { throw "Unsupported Windows target triple: $Triple (expected x86_64-* or aarch64-*)" }
        }
    }

    $HostArch = if (-not [string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
        $env:PROCESSOR_ARCHITEW6432.ToUpperInvariant()
    } elseif (-not [string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITECTURE)) {
        $env:PROCESSOR_ARCHITECTURE.ToUpperInvariant()
    } else {
        ""
    }
    switch ($HostArch) {
        "AMD64" { return @{ Desktop = "x86_64"; Server = "x86_64";  Conpty = "x64"   } }
        "X64"   { return @{ Desktop = "x86_64"; Server = "x86_64";  Conpty = "x64"   } }
        "ARM64" { return @{ Desktop = "arm64";  Server = "aarch64"; Conpty = "arm64" } }
        default { throw "Unsupported Windows host architecture: '$HostArch' (expected AMD64 or ARM64; pass -Target explicitly)" }
    }
}

# Inspect a PE32+ (64-bit Windows) executable's Import Directory and return imported DLL names.
function Get-PeImportedDlls {
    param([Parameter(Mandatory = $true)][string]$Path)

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 0x40 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "Not a valid PE executable (missing MZ header): $Path"
    }

    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    if ($peOffset -lt 0 -or ($peOffset + 24) -gt $bytes.Length) {
        throw "Corrupt PE header offset in $Path"
    }
    if ($bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45 -or $bytes[$peOffset + 2] -ne 0 -or $bytes[$peOffset + 3] -ne 0) {
        throw "Not a valid PE executable (missing PE\0\0 signature): $Path"
    }

    $coffOffset = $peOffset + 4
    $numSections = [BitConverter]::ToUInt16($bytes, $coffOffset + 2)
    $optHeaderSize = [BitConverter]::ToUInt16($bytes, $coffOffset + 16)
    $optOffset = $coffOffset + 20
    $magic = [BitConverter]::ToUInt16($bytes, $optOffset)

    # PE32 (0x10B) data directories start at optOffset + 96; PE32+ (0x20B) at optOffset + 112.
    $dataDirBase = if ($magic -eq 0x20B) { $optOffset + 112 } elseif ($magic -eq 0x10B) { $optOffset + 96 } else {
        throw "Unsupported PE optional header magic 0x$($magic.ToString('X')) in $Path"
    }

    # Import Table is DataDirectory[1] (offset +8 from dataDirBase).
    $importRva = [BitConverter]::ToUInt32($bytes, $dataDirBase + 8)
    $importSize = [BitConverter]::ToUInt32($bytes, $dataDirBase + 12)
    if ($importRva -eq 0 -or $importSize -eq 0) {
        return @()
    }

    $sections = New-Object System.Collections.Generic.List[hashtable]
    $secOffset = $optOffset + $optHeaderSize
    for ($i = 0; $i -lt $numSections; $i++) {
        $entry = $secOffset + ($i * 40)
        $virtSize = [BitConverter]::ToUInt32($bytes, $entry + 8)
        $virtAddr = [BitConverter]::ToUInt32($bytes, $entry + 12)
        $rawSize  = [BitConverter]::ToUInt32($bytes, $entry + 16)
        $rawPtr   = [BitConverter]::ToUInt32($bytes, $entry + 20)
        $span = [Math]::Max($virtSize, $rawSize)
        $sections.Add(@{ VirtAddr = $virtAddr; Span = $span; RawPtr = $rawPtr })
    }

    $rvaToOffset = {
        param([uint32]$rva)
        foreach ($s in $sections) {
            if ($rva -ge $s.VirtAddr -and $rva -lt ($s.VirtAddr + $s.Span)) {
                return [int]($rva - $s.VirtAddr + $s.RawPtr)
            }
        }
        return -1
    }

    $importOffset = & $rvaToOffset $importRva
    if ($importOffset -lt 0) {
        throw "Unable to map PE import table RVA in $Path"
    }

    $dlls = New-Object System.Collections.Generic.List[string]
    for ($cursor = $importOffset; ($cursor + 20) -le $bytes.Length; $cursor += 20) {
        $origFirstThunk = [BitConverter]::ToUInt32($bytes, $cursor)
        $nameRva        = [BitConverter]::ToUInt32($bytes, $cursor + 12)
        $firstThunk     = [BitConverter]::ToUInt32($bytes, $cursor + 16)
        if ($origFirstThunk -eq 0 -and $nameRva -eq 0 -and $firstThunk -eq 0) {
            break
        }
        $nameOffset = & $rvaToOffset $nameRva
        if ($nameOffset -lt 0) { continue }
        $end = $nameOffset
        while ($end -lt $bytes.Length -and $bytes[$end] -ne 0) { $end++ }
        if ($end -gt $nameOffset) {
            $dlls.Add([System.Text.Encoding]::ASCII.GetString($bytes, $nameOffset, $end - $nameOffset))
        }
    }
    return $dlls.ToArray()
}

function Assert-NoDynamicVcRuntime {
    param([Parameter(Mandatory = $true)][string[]]$Binaries)

    $forbiddenPattern = '^(vcruntime|msvcp)\d+(_\d+)?\.dll$'
    foreach ($bin in $Binaries) {
        $imports = Get-PeImportedDlls -Path $bin
        $bad = @($imports | Where-Object { $_ -imatch $forbiddenPattern })
        if ($bad.Count -gt 0) {
            throw "$(Split-Path $bin -Leaf) dynamically imports MSVC runtime DLL(s): $($bad -join ', '). Ensure .cargo/config.toml's +crt-static is active and RUSTFLAGS is unset."
        }
    }
}

# Build a deterministic portable ZIP using canonical '/' separators (required by `anterm-updater`).
function New-CanonicalZipFromDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$SourceDir,
        [Parameter(Mandatory = $true)][string]$DestinationZip
    )

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem

    if (Test-Path -LiteralPath $DestinationZip) {
        Remove-Item -LiteralPath $DestinationZip -Force
    }

    $fullSource = (Resolve-Path $SourceDir).Path.TrimEnd('\', '/')
    $files = Get-ChildItem -LiteralPath $fullSource -Recurse -File -Force | Sort-Object FullName

    $zip = [System.IO.Compression.ZipFile]::Open(
        $DestinationZip,
        [System.IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($file in $files) {
            $rel = $file.FullName.Substring($fullSource.Length + 1).Replace('\', '/')
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $zip,
                $file.FullName,
                $rel,
                [System.IO.Compression.CompressionLevel]::Optimal
            )
        }
    } finally {
        $zip.Dispose()
    }
}

$Arch = Resolve-ArchInfo -Triple $Target
$BinDir = if ([string]::IsNullOrWhiteSpace($Target)) {
    Join-Path $RootDir "target\release"
} else {
    Join-Path $RootDir "target\$Target\release"
}

if (-not $SkipBuild) {
    if (-not [string]::IsNullOrWhiteSpace($env:RUSTFLAGS)) {
        Write-Warning "RUSTFLAGS is set ('$($env:RUSTFLAGS)'), which overrides .cargo/config.toml (+crt-static). Dynamic VCRUNTIME check will fail if +crt-static is missing."
    }

    Write-Host "==> Building release binaries separately (version $Version, target: $(if ($Target) { $Target } else { 'host' }))..."
    Push-Location $RootDir
    try {
        # Build each binary crate individually so Cargo workspace feature unification
        # does not leak `anterm-app` features into `anterm-server` or `anterm-updater`.
        foreach ($pkg in @("anterm-app", "anterm-server", "anterm-updater")) {
            Write-Host "  -> cargo build --release -p $pkg"
            $cargoArgs = @("build", "--release", "-p", $pkg)
            if (-not [string]::IsNullOrWhiteSpace($Target)) {
                $cargoArgs += @("--target", $Target)
            }
            & cargo @cargoArgs
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build failed for $pkg (exit code $LASTEXITCODE)"
            }
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Host "==> Skipping cargo build (-SkipBuild); using binaries from $BinDir"
}

$AppExe     = Join-Path $BinDir "anterm-app.exe"
$ServerExe  = Join-Path $BinDir "anterm-server.exe"
$UpdaterExe = Join-Path $BinDir "anterm-updater.exe"

foreach ($bin in @($AppExe, $ServerExe, $UpdaterExe)) {
    if (-not (Test-Path -LiteralPath $bin -PathType Leaf)) {
        throw "Missing release binary: $bin"
    }
}

Write-Host "==> Checking static CRT linkage (no dynamic VCRUNTIME/MSVCP imports)..."
Assert-NoDynamicVcRuntime -Binaries @($AppExe, $ServerExe, $UpdaterExe)

$ConptyDir = Join-Path $RootDir "assets\windows\conpty\$($Arch.Conpty)"
$ConptyDll = Join-Path $ConptyDir "conpty.dll"
$OpenConsoleExe = Join-Path $ConptyDir "OpenConsole.exe"
$ConptyLicense = Join-Path $RootDir "assets\windows\conpty\LICENSE.txt"
$CompletionsDir = Join-Path $RootDir "assets\completions"

foreach ($asset in @($ConptyDll, $OpenConsoleExe, $ConptyLicense)) {
    if (-not (Test-Path -LiteralPath $asset -PathType Leaf)) {
        throw "Missing required Windows ConPTY asset: $asset"
    }
}
if (-not (Test-Path -LiteralPath $CompletionsDir -PathType Container)) {
    throw "Missing required completions directory: $CompletionsDir"
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$ZipName = "anterm-$Version-windows-$($Arch.Desktop).zip"
$ZipPath = Join-Path $OutDir $ZipName
$StandaloneServerName = "anterm-server-windows-$($Arch.Server).exe"
$StandaloneServerPath = Join-Path $OutDir $StandaloneServerName
$ChecksumsPath = Join-Path $OutDir "checksums.txt"
$Sha256SumsPath = Join-Path $OutDir "SHA256SUMS.txt"

$StageDir = Join-Path ([System.IO.Path]::GetTempPath()) ("anterm-win-stage-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $StageDir -Force | Out-Null

try {
    Write-Host "==> Staging portable payload ($($Arch.Desktop))..."

    # Exact `PORTABLE_MANAGED_ROOTS` layout verified by `crates/anterm-updater/src/windows.rs`.
    Copy-Item -LiteralPath $AppExe -Destination (Join-Path $StageDir "anterm-app.exe")
    Copy-Item -LiteralPath $UpdaterExe -Destination (Join-Path $StageDir "anterm-updater.exe")
    Copy-Item -LiteralPath $ConptyDll -Destination (Join-Path $StageDir "conpty.dll")
    Copy-Item -LiteralPath $OpenConsoleExe -Destination (Join-Path $StageDir "OpenConsole.exe")
    Copy-Item -LiteralPath (Join-Path $RootDir "LICENSE") -Destination (Join-Path $StageDir "LICENSE.txt")
    Copy-Item -LiteralPath $ConptyLicense -Destination (Join-Path $StageDir "LICENSE-ConPTY.txt")
    Copy-Item -LiteralPath (Join-Path $RootDir "README.md") -Destination (Join-Path $StageDir "README.md")

    # Exact ASCII payload "portable-v1" with no BOM and no trailing newline.
    [System.IO.File]::WriteAllBytes(
        (Join-Path $StageDir ".anterm-portable"),
        [System.Text.Encoding]::ASCII.GetBytes("portable-v1")
    )

    $StageServerDir = Join-Path $StageDir "server"
    New-Item -ItemType Directory -Path $StageServerDir -Force | Out-Null
    Copy-Item -LiteralPath $ServerExe -Destination (Join-Path $StageServerDir "anterm-server.exe")

    $StageCompletionsDir = Join-Path $StageDir "completions"
    New-Item -ItemType Directory -Path $StageCompletionsDir -Force | Out-Null
    Get-ChildItem -LiteralPath $CompletionsDir -Filter "*.json" -File |
        Copy-Item -Destination $StageCompletionsDir

    Write-Host "==> Creating $ZipPath ..."
    New-CanonicalZipFromDirectory -SourceDir $StageDir -DestinationZip $ZipPath

    Write-Host "==> Copying standalone server binary $StandaloneServerPath ..."
    Copy-Item -LiteralPath $ServerExe -Destination $StandaloneServerPath -Force

    # Generate lowercase SHA-256 checksums matching `anterm-updater` (`checksums.txt`) and `package-release.sh` (`SHA256SUMS.txt`).
    $ArtifactNames = @($ZipName, $StandaloneServerName)
    $ChecksumLines = foreach ($name in $ArtifactNames) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $OutDir $name) -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $name"
    }
    $ChecksumBody = ($ChecksumLines -join "`n") + "`n"
    [System.IO.File]::WriteAllText($ChecksumsPath, $ChecksumBody, [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText($Sha256SumsPath, $ChecksumBody, [System.Text.UTF8Encoding]::new($false))

    if (-not $SkipVerify) {
        Write-Host "==> Running smoke checks..."
        & $StandaloneServerPath --version
        if ($LASTEXITCODE -ne 0) {
            throw "anterm-server --version failed with exit code $LASTEXITCODE"
        }

        $VerifyStage = Join-Path ([System.IO.Path]::GetTempPath()) ("anterm-win-verify-" + [System.Guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $VerifyStage -Force | Out-Null
        try {
            & $UpdaterExe verify-portable $ZipPath $ChecksumsPath $ZipName $Version $VerifyStage
            if ($LASTEXITCODE -ne 0) {
                throw "anterm-updater verify-portable failed with exit code $LASTEXITCODE"
            }
            Write-Host "  -> anterm-updater verify-portable: OK"
        } finally {
            Remove-Item -LiteralPath $VerifyStage -Recurse -Force -ErrorAction SilentlyContinue
        }
    } else {
        Write-Host "==> Skipping smoke checks (-SkipVerify)"
    }
} finally {
    Remove-Item -LiteralPath $StageDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "==> Release artifacts in ${OutDir}:"
Get-Item -LiteralPath @($ZipPath, $StandaloneServerPath, $ChecksumsPath, $Sha256SumsPath) |
    Format-Table Name, @{Label="Size (MB)"; Expression={[math]::Round($_.Length / 1MB, 2)}}, LastWriteTime -AutoSize

Write-Host "==> checksums.txt:"
Get-Content -LiteralPath $ChecksumsPath
