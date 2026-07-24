#!/usr/bin/env sh
set -eu

canonical='https://rotnov.github.io/pycc/'

for required_file in \
  site/index.html \
  site/styles.css \
  site/site.js \
  site/og.png \
  site/robots.txt \
  site/sitemap.xml \
  site/llms.txt \
  site/404.html \
  site/.nojekyll
do
  test -f "$required_file"
done

grep -Fq "<link rel=\"canonical\" href=\"$canonical\">" site/index.html
grep -Fq 'name="description"' site/index.html
grep -Fq '<meta property="og:title"' site/index.html
grep -Fq '<meta property="og:image" content="https://rotnov.github.io/pycc/og.png">' site/index.html
grep -Fq '<meta name="twitter:card" content="summary_large_image">' site/index.html
grep -Fq '"@type": "SoftwareSourceCode"' site/index.html
grep -Fq "Sitemap: ${canonical}sitemap.xml" site/robots.txt
grep -Fq "<loc>${canonical}</loc>" site/sitemap.xml

if grep -R -nE '(localhost|127\.0\.0\.1|file://)' site; then
  echo "Website contains a local-only URL" >&2
  exit 1
fi

echo "Website checks passed."
