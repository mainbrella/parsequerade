#!/usr/bin/env bash
set -euo pipefail
umask 077

P8_FILE="${P8_FILE:-$HOME/Documents/AuthKey_522254PRQT.p8}"
ISSUER_ID="${ISSUER_ID:-69a6de86-27ba-47e3-e053-5b8c7c11a4d1}"
KEY_ID="${KEY_ID:-522254PRQT}"
API_BASE="${API_BASE:-https://parsequerade.mainbrella.com}"
CONNECTION_FILE="$(mktemp "${TMPDIR:-/tmp}/parsequerade-connection.XXXXXX")"
trap 'rm -f -- "$CONNECTION_FILE"' EXIT

if ! jq -n --arg issuerId "$ISSUER_ID" --arg keyId "$KEY_ID" \
  --rawfile privateKey "$P8_FILE" \
  '{issuerId: $issuerId, keyId: $keyId, privateKey: $privateKey}' |
  curl --fail-with-body --silent --show-error --connect-timeout 10 --max-time 40 \
    -X POST "${API_BASE%/}/v1/connections" \
    -H 'Content-Type: application/json' --data-binary @- > "$CONNECTION_FILE"; then
  cat "$CONNECTION_FILE" >&2
  printf '\nConnection setup failed.\n' >&2
  exit 1
fi

jq -e '.connectionId and .baseUrl and .apiKey' "$CONNECTION_FILE" > /dev/null
trap - EXIT
printf 'Connection created. Credentials saved to %s\n' "$CONNECTION_FILE"

BASE="$(jq -er '.baseUrl' "$CONNECTION_FILE")"
ASC_FACADE_KEY="$(jq -er '.apiKey' "$CONNECTION_FILE")"
curl --fail-with-body --silent --show-error --connect-timeout 10 --max-time 40 \
  "$BASE/apps?limit=50" \
  -H "Authorization: Bearer $ASC_FACADE_KEY" | jq .
