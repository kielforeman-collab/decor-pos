# First-run setup for decor-pos (Windows PowerShell).
#
# Creates `.env` from `.env.example` with fresh values — run once from
# the repo root:
#   .\setup.ps1
#
# Behaviour, by design:
# - If `.env` already exists, this prints a message and exits 1 WITHOUT
#   touching it. Delete (or move) the existing `.env` first if you
#   really want to regenerate it.
# - STORE_ID / ORG_ID get fresh UUIDs ([guid]::NewGuid()).
# - POSTGRES_PASSWORD gets a fresh random 32-hex-char value from a
#   cryptographic RNG — never a fixed default.
# - The generated password is printed ONCE, with a "save this" note.
#   It is not written anywhere else: save it in your password manager
#   now, because re-running this script will refuse (see above) and
#   the password cannot be recovered from the repo.
# - `.env` is gitignored — never commit it.

$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
$EnvFile = Join-Path $Root '.env'
$ExampleFile = Join-Path $Root '.env.example'

if (Test-Path $EnvFile) {
    Write-Host ".env already exists — refusing to overwrite it."
    Write-Host "Delete (or move) .env first if you really want to regenerate it."
    exit 1
}

if (-not (Test-Path $ExampleFile)) {
    Write-Host ".env.example not found in $Root — are you in the decor-pos repo root?"
    exit 1
}

$StoreId = [guid]::NewGuid().ToString()
$OrgId = [guid]::NewGuid().ToString()

# 16 crypto-random bytes -> 32 lowercase hex chars. Instance GetBytes
# (not the static Fill) so this works on Windows PowerShell 5.1 as
# well as PowerShell 7.
$Bytes = New-Object byte[] 16
$Rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
try { $Rng.GetBytes($Bytes) } finally { $Rng.Dispose() }
$DbPassword = ($Bytes | ForEach-Object { $_.ToString('x2') }) -join ''

$Content = Get-Content -Raw $ExampleFile
$Content = $Content -replace '(?m)^STORE_ID=.*$', "STORE_ID=$StoreId"
$Content = $Content -replace '(?m)^ORG_ID=.*$', "ORG_ID=$OrgId"
$Content = $Content -replace '(?m)^POSTGRES_PASSWORD=.*$', "POSTGRES_PASSWORD=$DbPassword"

# Write UTF-8 without BOM so Docker Compose / shells read it cleanly.
[System.IO.File]::WriteAllText($EnvFile, $Content, (New-Object System.Text.UTF8Encoding($false)))

Write-Host "Created .env from .env.example."
Write-Host "  STORE_ID=$StoreId"
Write-Host "  ORG_ID=$OrgId"
Write-Host ""
Write-Host "  POSTGRES_PASSWORD=$DbPassword"
Write-Host "  ^^^ SAVE THIS PASSWORD NOW (e.g. in your password manager)."
Write-Host "  It is printed only once and is not stored anywhere else."
Write-Host ""
Write-Host "Next: docker compose up --build -d   (see docs/self-hosting.md)"
