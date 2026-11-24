param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$')]
    [string] $Version,
    [string] $Compiler
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $repo 'dist'
$packages = [ordered]@{
    Lite = Join-Path $dist 'carrycode-lite-amd64.zip'
    Full = Join-Path $dist 'flashgpt-usb-amd64.zip'
}
foreach ($zip in $packages.Values) {
    if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) {
        throw "Missing $zip. Run xtask package and package-lite first."
    }
}

if (-not $Compiler) {
    $tools = Join-Path $dist 'installer-tools'
    $inno = Join-Path $tools 'InnoSetup7'
    $Compiler = Join-Path $inno 'ISCC.exe'
    if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
        $null = New-Item -ItemType Directory -Path $tools -Force
        $file = 'innosetup-7.1.1-x64.exe'
        $download = Join-Path $tools $file
        $url = "https://github.com/jrsoftware/issrc/releases/download/is-7_1_1/$file"
        $sha = '8a5a6711c3766a6e5fb74f280ba0e3551e719ce94538d901fdbd1f8f9920b3f0'
        if (-not (Test-Path -LiteralPath $download) -or (Get-FileHash -LiteralPath $download).Hash -ne $sha) {
            & curl.exe --fail --location --silent --show-error --retry 3 --output $download $url
            if ($LASTEXITCODE -ne 0) { throw 'Inno Setup download failed.' }
        }
        if ((Get-FileHash -LiteralPath $download).Hash -ne $sha) {
            throw 'Inno Setup checksum did not match.'
        }
        $process = Start-Process -FilePath $download -ArgumentList @(
            '/PORTABLE=1', '/CURRENTUSER', '/VERYSILENT', '/SUPPRESSMSGBOXES',
            '/NORESTART', '/NOICONS', ('/DIR="{0}"' -f $inno)
        ) -Wait -PassThru -WindowStyle Hidden
        if ($process.ExitCode -ne 0) { throw "Inno Setup bootstrap exited with code $($process.ExitCode)." }
    }
}
$Compiler = (Get-Item -LiteralPath $Compiler).FullName

Add-Type -AssemblyName System.IO.Compression.FileSystem
foreach ($edition in $packages.Keys) {
    $stage = Join-Path $dist ('installer-bundle-' + [Guid]::NewGuid().ToString('N'))
    try {
        [IO.Compression.ZipFile]::ExtractToDirectory($packages[$edition], $stage)
        $launcher = Join-Path $stage 'flashGPT.exe'
        $info = & $launcher --version
        if ($LASTEXITCODE -ne 0 -or $info.Trim() -ne "CarryCode $Version") {
            throw "The bundled Windows launcher does not match version $Version."
        }
        foreach ($path in 'profile', 'flashgpt.json', 'flashgpt-settings.json') {
            if (Test-Path -LiteralPath (Join-Path $stage $path)) {
                throw "Unexpected profile data in the input ZIP: $path"
            }
        }
        $recordedEdition = Get-Content -LiteralPath (Join-Path $stage 'manifests/edition.json') -Raw | ConvertFrom-Json
        if ($recordedEdition -cne $edition.ToLowerInvariant()) {
            throw "The ZIP edition does not match $edition."
        }
        if ($edition -eq 'Lite') {
            if (Test-Path -LiteralPath (Join-Path $stage 'bin')) {
                throw 'Unexpected CLI runtimes in the Lite ZIP.'
            }
        } else {
            foreach ($path in 'bin/windows-amd64/codex/bin/codex.exe', 'bin/windows-amd64/opencode/opencode.exe',
                'bin/linux-amd64/codex/bin/codex', 'bin/linux-amd64/opencode/opencode') {
                if (-not (Test-Path -LiteralPath (Join-Path $stage $path) -PathType Leaf)) {
                    throw "Missing CLI runtime in the Full ZIP: $path"
                }
            }
        }

        $script = Join-Path $PSScriptRoot 'flashgpt.iss'
        & $Compiler /Qp "/DAppVersion=$Version" "/DBundleDir=$stage" "/DEdition=$edition" "/O$dist" $script
        if ($LASTEXITCODE -ne 0) { throw "Inno Setup compiler exited with code $LASTEXITCODE." }
        $name = "CarryCode-$edition-Setup-Windows-x64.exe"
        $sha = (Get-FileHash -LiteralPath (Join-Path $dist $name)).Hash.ToLowerInvariant()
        Set-Content -LiteralPath (Join-Path $dist "$name.sha256") -Encoding ascii -Value "$sha  $name"
    } finally {
        $target = [IO.Path]::GetFullPath($stage)
        if ($target.StartsWith([IO.Path]::GetFullPath($dist) + '\', [StringComparison]::OrdinalIgnoreCase) -and
            (Test-Path -LiteralPath $target)) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }
}
