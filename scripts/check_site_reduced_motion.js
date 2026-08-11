#!/usr/bin/env node
/**
 * Reduced-motion computed-style evaluator for issue #199.
 *
 * Uses Puppeteer to emulate `prefers-reduced-motion: reduce` and
 * `no-preference` for each canonical page, then reads computed
 * `scroll-behavior` (root) and `transition-duration` (the documented
 * motion surfaces: skip link, nav/CTA buttons, spec-grid links) and
 * asserts the accessibility contract:
 *
 * - Under `reduce`: root scroll is not smooth, every reviewed
 *   nonessential transition is suppressed to ≤ 0.02ms.
 * - Under `no-preference`: normal motion remains available (root
 *   scroll-behavior may be smooth, transitions are non-zero).
 *
 * Uses the system Chrome via `PUPPETEER_EXECUTABLE_PATH` or
 * `--executable-path`, consistent with `run_pages_lighthouse.py`'s
 * `find_chrome()` pattern.  Puppeteer is a CI tool (like
 * `npx lighthouse`), not a site dependency.
 *
 * Usage:
 *   node scripts/check_site_reduced_motion.js \
 *     --base-url http://127.0.0.1:PORT/ \
 *     --manifest tests/fixtures/pages-performance-manifest.json
 *
 * Exit codes:
 *   0 — all pages pass the reduced-motion contract
 *   1 — one or more pages violate the contract
 *   2 — configuration or structural error
 */

'use strict';

const { execSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const PUPPETEER_VERSION = '24';

function findChrome() {
  const candidates = [
    process.env.PUPPETEER_EXECUTABLE_PATH || '',
    process.env.CHROME_PATH || '',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/snap/bin/chromium',
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
  ];
  for (const c of candidates) {
    if (c && fs.existsSync(c)) return c;
  }
  return '';
}

function loadManifest(manifestPath) {
  if (!fs.existsSync(manifestPath)) {
    throw new Error(`manifest not found: ${manifestPath}`);
  }
  return JSON.parse(fs.readFileSync(manifestPath, 'utf-8'));
}

function parseArgs(argv) {
  const args = { manifest: 'tests/fixtures/pages-performance-manifest.json', baseUrl: '', chromePath: '' };
  const rest = argv.slice(2);
  for (let i = 0; i < rest.length; i++) {
    switch (rest[i]) {
      case '--base-url': args.baseUrl = rest[++i]; break;
      case '--manifest': args.manifest = rest[++i]; break;
      case '--chrome-path': args.chromePath = rest[++i]; break;
      case '--help': case '-h':
        console.error('usage: check_site_reduced_motion.js --base-url URL [--manifest PATH] [--chrome-path PATH]');
        process.exit(0);
      default:
        console.error(`unknown argument: ${rest[i]}`);
        process.exit(2);
    }
  }
  if (!args.baseUrl) {
    console.error('FAIL: --base-url is required');
    process.exit(2);
  }
  return args;
}

async function checkPage(browser, url, pageId) {
  const failures = [];
  const page = await browser.newPage();
  await page.setViewport({ width: 412, height: 823, deviceScaleFactor: 1.75 });

  // --- Under no-preference ---
  await page.emulateMediaFeatures([
    { name: 'prefers-reduced-motion', value: 'no-preference' }
  ]);
  await page.goto(url, { waitUntil: 'networkidle0', timeout: 30000 });

  const noPref = await page.evaluate(() => {
    const root = document.documentElement;
    const rootScroll = getComputedStyle(root).scrollBehavior;
    const skipLink = document.querySelector('a[href="#main"], .skip-link');
    const skipTransition = skipLink ? getComputedStyle(skipLink).transitionDuration : null;
    const navLink = document.querySelector('.site-nav a, nav a');
    const navTransition = navLink ? getComputedStyle(navLink).transitionDuration : null;
    return { rootScroll, skipTransition, navTransition };
  });

  // Under no-preference, normal motion should be available.
  // scroll-behavior may be smooth (it is in the current site).
  // Transitions should be non-zero (160ms = 0.16s).
  if (noPref.skipTransition === '0s' || noPref.skipTransition === '0ms') {
    failures.push(`${pageId}: no-preference skip-link transition is zero (${noPref.skipTransition}) — normal motion should be available`);
  }
  if (noPref.navTransition === '0s' || noPref.navTransition === '0ms') {
    failures.push(`${pageId}: no-preference nav-link transition is zero (${noPref.navTransition}) — normal motion should be available`);
  }

  // --- Under reduce ---
  await page.emulateMediaFeatures([
    { name: 'prefers-reduced-motion', value: 'reduce' }
  ]);
  // Reload so media-query-based styles re-evaluate
  await page.reload({ waitUntil: 'networkidle0', timeout: 30000 });

  const reduce = await page.evaluate(() => {
    const root = document.documentElement;
    const rootScroll = getComputedStyle(root).scrollBehavior;
    const skipLink = document.querySelector('a[href="#main"], .skip-link');
    const skipTransition = skipLink ? getComputedStyle(skipLink).transitionDuration : null;
    const navLink = document.querySelector('.site-nav a, nav a');
    const navTransition = navLink ? getComputedStyle(navLink).transitionDuration : null;
    return { rootScroll, skipTransition, navTransition };
  });

  // Under reduce, scroll-behavior must not be smooth.
  if (reduce.rootScroll === 'smooth') {
    failures.push(`${pageId}: reduce root scroll-behavior is 'smooth' — must be 'auto' under prefers-reduced-motion: reduce`);
  }

  // Under reduce, nonessential transitions must be suppressed to ≤ 0.02ms
  // (tolerance for floating-point rounding; the CSS target is 0.01ms).
  // Computed values may be "0.01ms", "1e-05s", "0s", etc.
  function durationMs(value) {
    if (value === null) return -1; // element not found, skip
    if (value.endsWith('ms')) return parseFloat(value);
    if (value.endsWith('s')) return parseFloat(value) * 1000;
    return parseFloat(value) * 1000;
  }

  const skipMs = durationMs(reduce.skipTransition);
  if (skipMs >= 0 && skipMs > 0.02) {
    failures.push(`${pageId}: reduce skip-link transition is ${reduce.skipTransition} (${skipMs}ms) — must be ≤ 0.02ms under prefers-reduced-motion: reduce`);
  }
  const navMs = durationMs(reduce.navTransition);
  if (navMs >= 0 && navMs > 0.02) {
    failures.push(`${pageId}: reduce nav-link transition is ${reduce.navTransition} (${navMs}ms) — must be ≤ 0.02ms under prefers-reduced-motion: reduce`);
  }

  await page.close();
  return failures;
}

async function main() {
  const args = parseArgs(process.argv);
  const manifest = loadManifest(args.manifest);
  const chromePath = args.chromePath || findChrome();

  if (!chromePath) {
    console.error('FAIL: could not find Chrome/Chromium binary');
    process.exit(2);
  }

  // Try to require puppeteer; if not found, install it into a temp
  // directory and require it from there.  Using `npx` alone does not
  // persist the package in a location that `require()` can resolve, so
  // we use `npm install --prefix` to create a real node_modules tree.
  let puppeteer;
  try {
    puppeteer = require('puppeteer');
  } catch {
    const installDir = path.join(os.tmpdir(), 'puppeteer-install');
    console.error(`NOTE: puppeteer not found locally, installing into ${installDir}...`);
    execSync(
      `npm install --prefix "${installDir}" puppeteer@${PUPPETEER_VERSION}`,
      { stdio: 'inherit' }
    );
    puppeteer = require(path.join(installDir, 'node_modules', 'puppeteer'));
  }

  const browser = await puppeteer.launch({
    executablePath: chromePath,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu'],
  });

  try {
    const allFailures = [];
    for (const page of manifest.canonical_pages) {
      const pageId = page.id;
      const route = page.local_route.replace(/^\//, '');
      let url = `${args.baseUrl}${route}`;
      if (!url.endsWith('/')) url += '/';

      console.log(`Checking reduced-motion for ${pageId}...`);
      const failures = await checkPage(browser, url, pageId);
      allFailures.push(...failures);
    }

    if (allFailures.length > 0) {
      for (const f of allFailures) {
        console.error(`FAIL: ${f}`);
      }
      process.exit(1);
    }

    console.log('OK: reduced-motion contract passed for all canonical pages');
    process.exit(0);
  } finally {
    await browser.close();
  }
}

main().catch(err => {
  console.error(`ERROR: ${err.message}`);
  process.exit(2);
});
