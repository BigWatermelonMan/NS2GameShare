param(
    [string]$Name = 'NS2GameShare',
    [switch]$Gui,
    [string]$Keys = $env:NS2GAMESHARE_KEYS,
    [string]$PassphraseFile = $env:NS2GAMESHARE_PASSPHRASE,
    [string]$Port = 'auto',
    [string]$InputConfig,
    [ValidateRange(600,2000)][int]$ReceiveKbps = 1400,
    [switch]$Mute
)
$ErrorActionPreference = 'Stop'
$clientBinary = Join-Path $PSScriptRoot 'target\release\NS2GameShare.exe'
if ($Gui) { & $clientBinary; exit $LASTEXITCODE }
if (-not $Keys -or -not $PassphraseFile) {
    throw 'Use -Gui to configure your files, or specify -Keys and -PassphraseFile (or NS2GAMESHARE_KEYS / NS2GAMESHARE_PASSPHRASE).'
}
foreach ($requiredFile in @($clientBinary, $Keys, $PassphraseFile)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Required file is missing: $requiredFile"
    }
}
$clientArguments = @('play', '--name', $Name, '--port', $Port, '--keys', $Keys, '--passphrase-file', $PassphraseFile)
$clientArguments += @('--receive-kbps', $ReceiveKbps)
if ($Mute) { $clientArguments += '--mute' }
if ($InputConfig) { $clientArguments += @('--input-config', $InputConfig) }
& $clientBinary @clientArguments
exit $LASTEXITCODE
