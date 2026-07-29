import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, searchForWorkspaceRoot } from 'vite';

import tauriConfig from '../../crates/sahl-terminal/tauri.conf.json' with { type: 'json' };

export default defineConfig({
	// The About panel shows a version, and it has to be the one the installer carries. Read from
	// tauri.conf.json rather than duplicated here, so a release cannot ship a binary whose About
	// screen disagrees with its own package.
	define: { __APP_VERSION__: JSON.stringify(tauriConfig.version) },
	// The bundled fonts live in `packages/ui`, outside this app's root. Vite's dev server refuses to
	// serve files outside the project root by default, so every @font-face 403s and the page silently
	// renders in system fallbacks — the exact failure the bundling rule exists to prevent. Production
	// builds are unaffected (assets get copied), which is what makes this so easy to miss.
	server: { fs: { allow: [searchForWorkspaceRoot(process.cwd())] } },
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// The terminal is a single-page app served from inside the Tauri binary — there is no
			// server at the merchant's counter to render anything. `fallback` makes every route
			// resolve client-side from one HTML file.
			adapter: adapter({ fallback: 'index.html' })
		})
	]
});
