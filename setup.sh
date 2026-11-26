#!/usr/bin/env bash
# First-run setup for decor-pos (Linux / macOS).
#
# Creates `.env` from `.env.example` with fresh values — run once from
# the repo root:
#   ./setup.sh
#
# Behaviour, by design (same as setup.ps1, the Windows equivalent):
# - If `.env` already exists, this prints a message and exits 1 WITHOUT
#   touching it. Delete (or move) the existing `.env` first if you
#   really want to regenerate it.
# - STORE_ID / ORG_ID get fresh UUIDs (from /proc/sys/kernel/random/uuid
#   on Linux, else python3 uuid4, else uuidgen).
# - POSTGRES_PASSWORD gets a fresh random 32-hex-char value from
#   `openssl rand` (fallback: /dev/urandom) — never a fixed default.
# - The generated password is printed ONCE, with a "save this" note.
#   It is not written anywhere else: save it in your password manager
#   now, because re-running this script will refuse (see above) and
#   the password cannot be recovered from the repo.
# - `.env` is gitignored — never commit it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$ROOT/.env"
EXAMPLE_FILE="$ROOT/.env.example"

if [ -e "$ENV_FILE" ]; then
    echo ".env already exists — refusing to overwrite it."
    echo "Delete (or move) .env first if you really want to regenerate it."
    exit 1
fi

if [ ! -f "$EXAMPLE_FILE" ]; then
    echo ".env.example not found in $ROOT — are you in the decor-pos repo root?"
    exit 1
fi

new_uuid() {
    if [ -r /proc/sys/kernel/random/uuid ]; then
        cat /proc/sys/kernel/random/uuid
    elif command -v python3 >/dev/null 2>&1; then
        python3 -c 'import uuid; print(uuid.uuid4())'
    elif command -v uuidgen >/dev/null 2>&1; then
        uuidgen
    else
        echo "No UUID source found (need /proc/sys/kernel/random/uuid, python3, or uuidgen)." >&2
        exit 1
    fi
}

new_password() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 16
    else
        od -An -tx1 -N16 /dev/urandom | tr -d ' \n'
    fi
}

STORE_ID="$(new_uuid)"
ORG_ID="$(new_uuid)"
DB_PASSWORD="$(new_password)"

# UUIDs and hex passwords contain only [0-9a-f-], safe for sed replacement.
sed -e "s/^STORE_ID=.*/STORE_ID=$STORE_ID/" \
    -e "s/^ORG_ID=.*/ORG_ID=$ORG_ID/" \
    -e "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$DB_PASSWORD/" \
    "$EXAMPLE_FILE" > "$ENV_FILE"

echo "Created .env from .env.example."
echo "  STORE_ID=$STORE_ID"
echo "  ORG_ID=$ORG_ID"
echo ""
echo "  POSTGRES_PASSWORD=$DB_PASSWORD"
echo "  ^^^ SAVE THIS PASSWORD NOW (e.g. in your password manager)."
echo "  It is printed only once and is not stored anywhere else."
echo ""
echo "Next: docker compose up --build -d   (see docs/self-hosting.md)"
