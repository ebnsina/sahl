/**
 * Download the four bundled typefaces into `packages/ui/fonts/` and generate `src/fonts.css`.
 *
 * These fonts are *bundled*, never loaded from a CDN. The terminal is offline-first: a webfont
 * request that fails is a register rendering in Times New Roman during a rush, and the shops this
 * product targets are exactly the ones with unreliable connectivity. Committing the woff2 files is
 * the point, not an oversight.
 *
 * Only the subsets each font is actually responsible for are fetched — Latin from Mona Sans and
 * Geist Mono, Bengali from Anek Bangla, Arabic from IBM Plex Sans Arabic — which keeps the terminal
 * binary small on the low-end hardware these merchants buy.
 *
 * Run with: pnpm --filter @sahl/ui fetch-fonts
 */

import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const PACKAGE_ROOT = join(HERE, '..');
const FONT_DIR = join(PACKAGE_ROOT, 'fonts');
const CSS_PATH = join(PACKAGE_ROOT, 'src', 'fonts.css');

/** Google's API serves woff2 only to browser user agents. */
const BROWSER_UA =
	'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36';

/**
 * `subsets` names the unicode subsets to keep, matching the `/* comment *\/` labels Google emits
 * above each @font-face block.
 */
const FAMILIES = [
	{
		family: 'Mona Sans',
		query: 'Mona+Sans:wght@200..900',
		slug: 'mona-sans',
		subsets: ['latin', 'latin-ext'],
		role: 'UI text'
	},
	{
		family: 'Geist Mono',
		query: 'Geist+Mono:wght@100..900',
		slug: 'geist-mono',
		subsets: ['latin', 'latin-ext'],
		role: 'all numerics, tabular'
	},
	{
		family: 'Tiro Bangla',
		query: 'Tiro+Bangla',
		slug: 'tiro-bangla',
		subsets: ['bengali'],
		// NOTE: Tiro Bangla ships a single weight (400) and no bold. Anywhere the UI asks for 500/600
		// — buttons, labels, totals — Bangla text will be synthetically emboldened by the browser.
		role: 'Bangla (single weight 400)'
	},
	{
		family: 'IBM Plex Sans Arabic',
		query: 'IBM+Plex+Sans+Arabic:wght@400;500;600;700',
		slug: 'ibm-plex-sans-arabic',
		subsets: ['arabic'],
		role: 'Arabic'
	}
];

/** Parse Google's generated CSS into one record per @font-face block. */
function parseFaces(css) {
	const faces = [];
	// Each block is preceded by a `/* subset */` comment naming its unicode subset.
	const pattern = /\/\*\s*([\w-]+)\s*\*\/\s*@font-face\s*\{([^}]*)\}/g;
	for (const [, subset, body] of css.matchAll(pattern)) {
		const url = body.match(/src:\s*url\(([^)]+)\)/)?.[1];
		const unicodeRange = body.match(/unicode-range:\s*([^;]+);/)?.[1]?.trim();
		const weight = body.match(/font-weight:\s*([^;]+);/)?.[1]?.trim() ?? '400';
		const style = body.match(/font-style:\s*([^;]+);/)?.[1]?.trim() ?? 'normal';
		if (url) faces.push({ subset, url, unicodeRange, weight, style });
	}
	return faces;
}

async function fetchText(url) {
	const response = await fetch(url, { headers: { 'User-Agent': BROWSER_UA } });
	if (!response.ok) {
		throw new Error(`GET ${url} failed with HTTP ${response.status}`);
	}
	return response.text();
}

async function fetchBinary(url) {
	const response = await fetch(url, { headers: { 'User-Agent': BROWSER_UA } });
	if (!response.ok) {
		throw new Error(`GET ${url} failed with HTTP ${response.status}`);
	}
	return Buffer.from(await response.arrayBuffer());
}

async function main() {
	await mkdir(FONT_DIR, { recursive: true });

	const blocks = [];
	let downloaded = 0;

	for (const entry of FAMILIES) {
		const css = await fetchText(
			`https://fonts.googleapis.com/css2?family=${entry.query}&display=block`
		);
		const faces = parseFaces(css).filter((face) => entry.subsets.includes(face.subset));

		if (faces.length === 0) {
			throw new Error(
				`No matching subsets for ${entry.family}. Wanted ${entry.subsets.join(', ')}; ` +
					`the API returned ${[...new Set(parseFaces(css).map((f) => f.subset))].join(', ')}.`
			);
		}

		for (const face of faces) {
			// Weights like "200 900" (variable ranges) need flattening for a filename.
			const weightSlug = face.weight.replaceAll(' ', '-');
			const filename = `${entry.slug}-${face.subset}-${weightSlug}.woff2`;
			await writeFile(join(FONT_DIR, filename), await fetchBinary(face.url));
			downloaded += 1;

			blocks.push(
				[
					`@font-face {`,
					`\tfont-family: '${entry.family}';`,
					`\tfont-style: ${face.style};`,
					`\tfont-weight: ${face.weight};`,
					// `block` rather than `swap`: a brief invisible render beats a visible reflow of
					// every price on the screen mid-transaction.
					`\tfont-display: block;`,
					`\tsrc: url('../fonts/${filename}') format('woff2');`,
					face.unicodeRange ? `\tunicode-range: ${face.unicodeRange};` : null,
					`}`
				]
					.filter(Boolean)
					.join('\n')
			);
		}

		console.log(`  ${entry.family.padEnd(22)} ${faces.length} file(s)  — ${entry.role}`);
	}

	const header = [
		'/*',
		' * GENERATED by scripts/fetch-fonts.mjs — do not edit by hand.',
		' *',
		' * Self-hosted and bundled, never CDN-loaded. The terminal is offline-first: a webfont',
		' * request that fails is a register rendering in Times New Roman during a rush.',
		' */',
		''
	].join('\n');

	await writeFile(CSS_PATH, `${header}\n${blocks.join('\n\n')}\n`);
	console.log(`\n${downloaded} font files written to packages/ui/fonts/`);
	console.log(`Generated ${CSS_PATH}`);
}

main().catch((error) => {
	console.error(`\nFont fetch failed: ${error.message}`);
	process.exit(1);
});
