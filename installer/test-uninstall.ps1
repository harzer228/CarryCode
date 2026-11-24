param(
    [Parameter(Mandatory)]
    [string] $Version
)

$ErrorActionPreference = 'Stop'
$dist = Join-Path (Split-Path -Parent $PSScriptRoot) 'dist'
$registryPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\flashGPT.Portable_is1'
if (Test-Path -LiteralPath $registryPath) {
    throw 'Use a Windows test account without an installed CarryCode uninstaller.'
}

foreach ($edition in 'Lite', 'Full') {
    $setup = Join-Path $dist "CarryCode-$edition-Setup-Windows-x64.exe"
    $destination = Join-Path $dist ("uninstall-test-$edition-" + [Guid]::NewGuid().ToString('N'))
    $data = @('flashgpt.json', 'flashgpt-settings.json', 'profile/snapshot-0.age', 'projects/demo.txt')
    $hashes = @{}
    foreach ($relative in $data) {
        $file = Join-Path $destination $relative
        $null = New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force
        Set-Content -LiteralPath $file -Value "Synthetic data for $relative"
        $hashes[$relative] = (Get-FileHash -LiteralPath $file).Hash
    }

    $install = Start-Process -FilePath $setup -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOICONS', ('/DIR="{0}"' -f $destination)
    ) -Wait -PassThru -WindowStyle Hidden
    if ($install.ExitCode -ne 0) { throw "$edition setup exited with $($install.ExitCode)." }

    $launcher = Join-Path $destination 'flashGPT.exe'
    if ((& $launcher --version) -ne "CarryCode $Version" -or $LASTEXITCODE -ne 0) {
        throw "$edition setup installed the wrong launcher."
    }
    $installedEdition = Get-Content -LiteralPath (Join-Path $destination 'manifests/edition.json') -Raw | ConvertFrom-Json
    if ($installedEdition -cne $edition.ToLowerInvariant()) {
        throw "$edition setup installed the wrong edition marker."
    }
    $uninstaller = @(Get-ChildItem -LiteralPath $destination -Filter 'unins*.exe' -File)
    if ($uninstaller.Count -ne 1) { throw "$edition setup did not create one uninstaller." }
    $entry = Get-ItemProperty -LiteralPath $registryPath
    if ($entry.DisplayName -ne "CarryCode $edition" -or $entry.DisplayVersion -ne $Version -or
        $entry.UninstallString.Trim('"') -ne $uninstaller[0].FullName) {
        throw "$edition setup registered the wrong uninstall entry."
    }
    $remove = Start-Process -FilePath $uninstaller[0].FullName -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    ) -Wait -PassThru -WindowStyle Hidden
    if ($remove.ExitCode -ne 0) { throw "$edition uninstall exited with $($remove.ExitCode)." }

    $remainingFiles = @(Get-ChildItem -LiteralPath $destination -Recurse -File)
    if ($remainingFiles.Count -ne $data.Count -or (Test-Path -LiteralPath $registryPath)) {
        throw "$edition uninstall left installed files or its Windows registration."
    }
    foreach ($relative in $data) {
        $file = Join-Path $destination $relative
        if ((Get-FileHash -LiteralPath $file).Hash -ne $hashes[$relative]) {
            throw "$edition uninstall changed $relative."
        }
    }
    Write-Output "$edition uninstall preserved the profile, settings, and project."
}
