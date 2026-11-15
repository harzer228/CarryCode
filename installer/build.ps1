param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$')]
    [string] $Version,
    [string] $Compiler
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $repo 'dist'
$zip = Join-Path $dist 'flashgpt-usb-amd64.zip'
if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) {
    throw 'Run xtask package first or put the matching release ZIP in dist/.'
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
$stage = Join-Path $dist ('installer-bundle-' + [Guid]::NewGuid().ToString('N'))
try {
    [IO.Compression.ZipFile]::ExtractToDirectory($zip, $stage)
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

    $script = Join-Path $PSScriptRoot 'flashgpt.iss'
    & $Compiler /Qp "/DAppVersion=$Version" "/DBundleDir=$stage" "/O$dist" $script
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup compiler exited with code $LASTEXITCODE." }
    $name = 'flashGPT-setup-windows-x64.exe'
    $sha = (Get-FileHash -LiteralPath (Join-Path $dist $name)).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $dist "$name.sha256") -Encoding ascii -Value "$sha  $name"
} finally {
    $target = [IO.Path]::GetFullPath($stage)
    if ($target.StartsWith([IO.Path]::GetFullPath($dist) + '\', [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $target)) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}
