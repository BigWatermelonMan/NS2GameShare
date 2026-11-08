$ErrorActionPreference='Stop'
$outputPath=Join-Path $PSScriptRoot 'build'
New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
Push-Location $PSScriptRoot
try {
    & sdcc -mmcs51 --model-small --code-size 0x2000 --xram-loc 0x2000 --xram-size 0x1c00 --out-fmt-ihx -o (Join-Path $outputPath 'basic-fx2.ihx') main.c
    if ($LASTEXITCODE -ne 0) { throw 'SDCC build failed' }
} finally { Pop-Location }
