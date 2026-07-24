#!/bin/sh
set -eu

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
test_claude_config=$(mktemp -d "${TMPDIR:-/tmp}/pycc-claude-config.XXXXXX")
marketplaces_root="$test_claude_config/inline-marketplaces"
enabled_plugins_file="$test_claude_config/enabled-plugins.txt"
installed_plugins_file="$test_claude_config/installed-plugins.json"

cleanup() {
  rm -rf -- "$test_claude_config"
}
trap cleanup EXIT HUP INT TERM

if ! command -v claude >/dev/null 2>&1; then
  echo "error: Claude Code CLI is required for marketplace validation" >&2
  exit 1
fi

doctor_output=$(
  cd "$repo_root"
  env CLAUDE_CONFIG_DIR="$test_claude_config" claude doctor 2>&1
)
printf '%s\n' "$doctor_output"
if printf '%s\n' "$doctor_output" | grep -q "Invalid settings"; then
  echo "error: Claude Code rejected the project settings" >&2
  exit 1
fi

mkdir -p "$marketplaces_root"
python3 - "$repo_root/.claude/settings.json" \
  "$marketplaces_root" "$enabled_plugins_file" <<'PY'
import json
import sys
from pathlib import Path

settings_path = Path(sys.argv[1])
marketplaces_root = Path(sys.argv[2])
enabled_plugins_path = Path(sys.argv[3])
settings = json.loads(settings_path.read_text(encoding="utf-8"))
for name, configuration in settings["extraKnownMarketplaces"].items():
    source = configuration["source"]
    if source.get("source") != "settings":
        raise SystemExit(f"{name}: marketplace is not inline and immutable")
    marketplace_path = marketplaces_root / name / ".claude-plugin" / "marketplace.json"
    marketplace_path.parent.mkdir(parents=True)
    marketplace = {
        "name": source["name"],
        "owner": {"name": "pycc maintainers"},
        "description": "Pinned repository agent tooling.",
        "plugins": source["plugins"],
    }
    marketplace_path.write_text(
        json.dumps(marketplace, indent=2) + "\n",
        encoding="utf-8",
    )
enabled = [
    coordinate
    for coordinate, value in settings["enabledPlugins"].items()
    if value is True
]
enabled_plugins_path.write_text("\n".join(sorted(enabled)) + "\n", encoding="utf-8")
PY

for marketplace_root in "$marketplaces_root"/*; do
  env CLAUDE_CONFIG_DIR="$test_claude_config" \
    claude plugin validate --strict "$marketplace_root"
  env CLAUDE_CONFIG_DIR="$test_claude_config" \
    claude plugin marketplace add "$marketplace_root"
done

while IFS= read -r coordinate; do
  env CLAUDE_CONFIG_DIR="$test_claude_config" \
    claude plugin install --scope user "$coordinate"
done <"$enabled_plugins_file"

env CLAUDE_CONFIG_DIR="$test_claude_config" \
  claude plugin list --json >"$installed_plugins_file"
python3 - "$enabled_plugins_file" "$installed_plugins_file" <<'PY'
import json
import sys
from pathlib import Path

expected = {
    line
    for line in Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
    if line
}
payload = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
entries = payload.get("installed", []) if isinstance(payload, dict) else payload
if not isinstance(entries, list):
    raise SystemExit("Claude plugin list must be an array")
installed = {
    plugin["id"]
    for plugin in entries
    if isinstance(plugin, dict) and isinstance(plugin.get("id"), str)
}
missing = sorted(expected - installed)
if missing:
    raise SystemExit("enabled plugins were not installed: " + ", ".join(missing))
PY

rust_analyzer_details=$(
  env CLAUDE_CONFIG_DIR="$test_claude_config" \
    claude plugin details "rust-analyzer-lsp@pycc-official-pinned"
)
printf '%s\n' "$rust_analyzer_details"
if ! printf '%s\n' "$rust_analyzer_details" |
  grep -Fq "LSP servers (1)  rust-analyzer"; then
  echo "error: rust-analyzer-lsp lost its reviewed LSP component metadata" >&2
  exit 1
fi

echo "Claude marketplaces: manifests and pinned plugin installs are valid"
