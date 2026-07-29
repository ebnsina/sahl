// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}

	/**
	 * Injected by Vite from tauri.conf.json — see `define` in vite.config.ts.
	 *
	 * Inside `declare global` because this file is a module: a bare `declare const` at the top level
	 * would be scoped to the module and invisible to every component.
	 */
	const __APP_VERSION__: string;
}

export {};
