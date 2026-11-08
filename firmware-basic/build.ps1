param([string]$Port, [switch]$Flash,
      [string]$BuildDirectory = (Join-Path $PSScriptRoot 'build'),
      [string]$Sdkconfig)
$ErrorActionPreference='Stop'
if (-not $env:IDF_PATH) { throw 'Set IDF_PATH to your ESP-IDF v5.5.6 installation.' }
. "$env:IDF_PATH\export.ps1" | Out-Null
if (-not (Get-Command xtensa-esp32-elf-gcc -ErrorAction SilentlyContinue)) {
    # An IDF installation initially configured for C5 can omit Xtensa from PATH.
    $toolRoot = $env:IDF_TOOLS_PATH
    if (-not $toolRoot) { $toolRoot = Join-Path $env:USERPROFILE '.espressif' }
    $toolCatalog = Get-Content (Join-Path $env:IDF_PATH 'tools\tools.json') -Raw | ConvertFrom-Json
    $tool = $toolCatalog.tools | Where-Object name -eq 'xtensa-esp-elf'
    $version = ($tool.versions | Where-Object status -eq 'recommended').name
    $compilerDirectory = Join-Path $toolRoot "tools\xtensa-esp-elf\$version\xtensa-esp-elf\bin"
    if (Test-Path -LiteralPath (Join-Path $compilerDirectory 'xtensa-esp32-elf-gcc.exe')) {
        $env:Path = $compilerDirectory + [IO.Path]::PathSeparator + $env:Path
    }
}
if (-not (Get-Command xtensa-esp32-elf-gcc -ErrorAction SilentlyContinue)) { throw 'ESP-IDF v5.5.6 Xtensa toolchain is required.' }
$buildArgs=@('-C',$PSScriptRoot,'-B',$BuildDirectory,'-DIDF_TARGET=esp32')
if ($Sdkconfig) { $buildArgs += "-DSDKCONFIG=$Sdkconfig" }
if ($Flash) {
    if (-not $Port) { throw 'Specify the Basic USB-UART port explicitly with -Port. Live playback uses FX2, not USB-UART.' }
    $buildArgs+=@('-p',$Port,'-b','115200','flash')
} else { $buildArgs+='build' }
idf.py @buildArgs
exit $LASTEXITCODE
