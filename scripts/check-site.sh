#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'
site_dir=${SITE_DIR:-site}

for required_file in \
  index.html \
  styles.css \
  site.js \
  og.png \
  robots.txt \
  sitemap.xml \
  llms.txt \
  404.html \
  .nojekyll
do
  test -f "$site_dir/$required_file"
done

test -s "$site_dir/og.png"

assert_once() {
  expected=$1
  file=$2
  count=$(grep -Fc "$expected" "$file" || true)
  if [ "$count" -ne 1 ]; then
    echo "Expected exactly one occurrence in $file: $expected" >&2
    exit 1
  fi
}

index="$site_dir/index.html"
assert_once '<title>pycc — AOT compiler for typed Python to native binaries</title>' "$index"
assert_once 'name="description"' "$index"
assert_once "<link rel=\"canonical\" href=\"$canonical\">" "$index"
assert_once '<link rel="sitemap" type="application/xml" href="sitemap.xml">' "$index"
assert_once '<meta name="robots" content="index, follow, max-image-preview:large">' "$index"

assert_once '<meta property="og:type" content="website">' "$index"
assert_once '<meta property="og:url" content="https://rotnov.github.io/pycc/">' "$index"
assert_once '<meta property="og:title"' "$index"
assert_once 'property="og:description"' "$index"
assert_once '<meta property="og:image" content="https://rotnov.github.io/pycc/og.png">' "$index"
assert_once '<meta property="og:image:alt"' "$index"

assert_once '<meta name="twitter:card" content="summary_large_image">' "$index"
assert_once '<meta name="twitter:title"' "$index"
assert_once 'name="twitter:description"' "$index"
assert_once '<meta name="twitter:image" content="https://rotnov.github.io/pycc/og.png">' "$index"
assert_once '<meta name="twitter:image:alt"' "$index"

assert_once '"@type": "SoftwareSourceCode"' "$index"
assert_once '"codeRepository": "https://github.com/rotnov/pycc"' "$index"

assert_once "Sitemap: ${canonical}sitemap.xml" "$site_dir/robots.txt"
assert_once "<loc>${canonical}</loc>" "$site_dir/sitemap.xml"

if grep -R -nE '(localhost|127\.0\.0\.1|file://)' "$site_dir"; then
  echo "Website contains a local-only URL" >&2
  exit 1
fi

echo "Website checks passed."
