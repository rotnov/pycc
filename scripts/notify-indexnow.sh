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

sitemap_sha256=$(printf '%s' "$payload" | sha256sum | cut -d' ' -f1)
url_count=$(printf '%s' "$payload" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['urlList']))")
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
endpoint_host=$(printf '%s' "$endpoint" | sed 's|^[a-z]*://||; s|/.*||')
deployed_commit=${GITHUB_SHA:-unknown}
retry_config="retries=${retry_count}"

# Capture both the HTTP status code and the number of curl attempts.
# %{http_code} gives the final HTTP status; %{num_redirects} is not the
# same as retry attempts.  We use a write-out format that captures
# http_code and the exit status separately so we can classify failures.
curl_exit=0
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
  "$endpoint" 2>/dev/null) || curl_exit=$?

# Classify the outcome into a sanitized failure class.  The class is
# derived from the curl exit status and the HTTP response code, never
# from response bodies or headers that might contain sensitive data.
# Per the IndexNow protocol:
#   200 = URL set submitted successfully
#   202 = URL set received; key validation pending
#   400 = Invalid request format
#   403 = Invalid/unavailable key
#   422 = URL/host/key-scope mismatch
#   429 = Too many requests
# Receipt (200/202) is not proof of crawl or indexing.
# Note: --fail-with-body causes curl to exit non-zero for HTTP >= 400
# even though it still captures the response code.  So we classify by
# the HTTP response code when we have one, and by the curl exit code
# only when no HTTP response was received at all.
if [ -z "$response_code" ] || [ "$response_code" = "000" ]; then
  # curl failed before getting any HTTP response — classify by exit code.
  # %{http_code} returns "000" when no response was received.
  case "$curl_exit" in
    6)   failure_class="dns_error" ;;
    7)   failure_class="connect_error" ;;
    28)  failure_class="timeout" ;;
    35|52|56|60) failure_class="tls_error" ;;
    *)   failure_class="network_error" ;;
  esac
  accepted_class="failed"
  response_code="none"
else
  case "$response_code" in
    200)
      accepted_class="submitted"
      failure_class="none"
      ;;
    202)
      accepted_class="key_validation_pending"
      failure_class="none"
      ;;
    400)
      accepted_class="failed"
      failure_class="bad_request"
      ;;
    403)
      accepted_class="failed"
      failure_class="forbidden"
      ;;
    422)
      accepted_class="failed"
      failure_class="scope_mismatch"
      ;;
    429)
      accepted_class="failed"
      failure_class="rate_limited"
      ;;
    *)
      accepted_class="failed"
      failure_class="http_error_${response_code}"
      ;;
  esac
fi

# Emit the structured one-line result record on every terminal path.
# Never log the full server response body, credentials, or runner state.
# The IndexNow verification key is public by protocol design.
result_line="IndexNow result: timestamp=${timestamp} endpoint_host=${endpoint_host} method=POST url_count=${url_count} sitemap_sha256=${sitemap_sha256} deployed_commit=${deployed_commit} http_status=${response_code} accepted_class=${accepted_class} failure_class=${failure_class} ${retry_config} curl_exit=${curl_exit}"
printf '%s\n' "$result_line"

# Publish a concise job summary so a reader can identify the trusted
# commit, URL count, sitemap digest, response class, and delivery
# outcome without downloading raw logs.
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '### IndexNow notification\n\n'
    printf '| Field | Value |\n|---|---|\n'
    printf '| Timestamp (UTC) | %s |\n' "$timestamp"
    printf '| Endpoint host | %s |\n' "$endpoint_host"
    printf '| URL count | %s |\n' "$url_count"
    printf '| Sitemap SHA-256 | `%s` |\n' "$sitemap_sha256"
    printf '| Deployed commit | `%s` |\n' "$deployed_commit"
    printf '| HTTP status | %s |\n' "$response_code"
    printf '| Accepted class | %s |\n' "$accepted_class"
    printf '| Failure class | %s |\n' "$failure_class"
    printf '| Retry config | %s |\n' "$retry_config"
    printf '| curl exit | %s |\n\n' "$curl_exit"
    printf 'Receipt (200/202) proves the endpoint accepted the batch, '
    printf '**not** that URLs were crawled or indexed. '
    printf 'Crawl and indexing states are separate observations.\n'
  } >> "$GITHUB_STEP_SUMMARY"
fi

case "$response_code" in
  200|202)
    ;;
  *)
    echo "IndexNow endpoint returned unexpected HTTP status: $response_code (failure_class=${failure_class})" >&2
    exit 1
    ;;
esac
