/**
 * The terminal runs entirely client-side inside the Tauri webview.
 *
 * There is no server at the merchant's counter, and there must not be a dependency on one: the
 * whole promise of this product is that the register keeps selling when the internet does not.
 * Server-side rendering would make the first paint depend on a network round trip, which is exactly
 * the failure we are engineering against.
 */
export const ssr = false;
export const prerender = false;
