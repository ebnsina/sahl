import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, searchForWorkspaceRoot } from 'vite';

export default defineConfig({
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
			adapter: adapter()
		})
	]
});
