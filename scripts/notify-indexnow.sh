#!/usr/bin/env sh
set -eu

unset CDPATH
repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd)
canonical='https://rotnov.github.io/pycc/'
key='3361fe03d0f44ab7cdbb1a3ce1461821'
key_location="${canonical}${key}.txt"
endpoint=${INDEXNOW_ENDPOINT:-https://api.indexnow.org/indexnow}
sitemap=${INDEXNOW_SITEMAP:-"$repo_root/site/sitemap.xml"}
retry_count=${INDEXNOW_RETRY_COUNT:-3}
connect_timeout=${INDEXNOW_CONNECT_TIMEOUT_SECONDS:-10}
max_time=${INDEXNOW_MAX_TIME_SECONDS:-30}

case "$retry_count" in
  ''|*[!0-9]*)
    echo "INDEXNOW_RETRY_COUNT must be a non-negative integer" >&2
    exit 2
    ;;
esac
case "$connect_timeout" in
  ''|*[!0-9]*|0)
    echo "INDEXNOW_CONNECT_TIMEOUT_SECONDS must be a positive integer" >&2
    exit 2
    ;;
esac
case "$max_time" in
  ''|*[!0-9]*|0)
    echo "INDEXNOW_MAX_TIME_SECONDS must be a positive integer" >&2
    exit 2
    ;;
esac

payload=$(python3 - "$sitemap" "$canonical" "$key" "$key_location" <<'PY'
import json
from pathlib import Path
import sys
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET

sitemap_path = Path(sys.argv[1])
canonical = sys.argv[2]
key = sys.argv[3]
key_location = sys.argv[4]
namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}

canonical_parts = urlsplit(canonical)
if canonical_parts.scheme != "https" or not canonical_parts.hostname:
    raise SystemExit("IndexNow canonical URL must be an absolute HTTPS URL")

try:
    root = ET.parse(sitemap_path).getroot()
except (OSError, ET.ParseError) as error:
    raise SystemExit(f"Could not read IndexNow sitemap: {error}") from error

entries = root.findall("s:url", namespace)
if not entries:
    raise SystemExit("IndexNow sitemap contains no canonical URLs")
urls = []
for entry in entries:
    locations = entry.findall("s:loc", namespace)
    if len(locations) != 1:
        raise SystemExit(
            "IndexNow sitemap URL entry must contain exactly one loc"
        )
    urls.append((locations[0].text or "").strip())
if len(urls) > 10_000:
    raise SystemExit("IndexNow batch exceeds the 10,000 URL protocol limit")
if len(urls) != len(set(urls)):
    raise SystemExit("IndexNow sitemap contains duplicate canonical URLs")

for url in urls:
    parts = urlsplit(url)
    decoded_path = parts.path
    for _ in range(5):
        next_path = unquote(decoded_path)
        if next_path == decoded_path:
            break
        decoded_path = next_path
    else:
        raise SystemExit(
            f"IndexNow sitemap URL has excessive percent encoding: {url}"
        )
    has_dot_segment = any(
        segment in {".", ".."} for segment in decoded_path.split("/")
    )
    if (
        parts.scheme != canonical_parts.scheme
        or parts.hostname != canonical_parts.hostname
        or parts.port != canonical_parts.port
        or parts.username is not None
        or parts.password is not None
        or "\\" in decoded_path
        or not decoded_path.startswith(canonical_parts.path)
        or has_dot_segment
        or parts.query
        or parts.fragment
    ):
        raise SystemExit(
            f"IndexNow sitemap URL is outside the verified project path: {url}"
        )

print(
    json.dumps(
        {
            "host": canonical_parts.hostname,
            "key": key,
            "keyLocation": key_location,
            "urlList": urls,
        },
        ensure_ascii=False,
        separators=(",", ":"),
    )
)
PY
)

if [ "${INDEXNOW_DRY_RUN:-}" = "1" ]; then
  printf '%s\n' "$payload"
  exit 0
fi

response_code=$(curl \
  --disable \
  --connect-timeout "$connect_timeout" \
  --data-binary "$payload" \
  --fail-with-body \
  --header 'Content-Type: application/json; charset=utf-8' \
  --location \
  --max-time "$max_time" \
  --request POST \
  --retry "$retry_count" \
  --retry-all-errors \
  --show-error \
  --silent \
  --output /dev/null \
  --write-out '%{http_code}' \
  "$endpoint")

case "$response_code" in
  200|202)
    ;;
  *)
    echo "IndexNow endpoint returned unexpected HTTP status: $response_code" >&2
    exit 1
    ;;
esac
