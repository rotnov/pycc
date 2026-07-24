#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'
key='3361fe03d0f44ab7cdbb1a3ce1461821'
key_location="${canonical}${key}.txt"
endpoint=${INDEXNOW_ENDPOINT:-https://api.indexnow.org/indexnow}

if [ "${INDEXNOW_DRY_RUN:-}" = "1" ]; then
  printf 'url=%s\nkey=%s\nkeyLocation=%s\n' "$canonical" "$key" "$key_location"
  exit 0
fi

curl \
  --fail-with-body \
  --get \
  --location \
  --retry 3 \
  --retry-all-errors \
  --show-error \
  --silent \
  --data-urlencode "url=$canonical" \
  --data-urlencode "key=$key" \
  --data-urlencode "keyLocation=$key_location" \
  "$endpoint"
