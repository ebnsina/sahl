import { redirect } from '@sveltejs/kit';
import { TOKEN_COOKIE, day, outlets } from '$lib/api';
import type { PageServerLoad } from './$types';

/**
 * Load the shop.
 *
 * Server-side so the token never reaches the browser, and so a merchant on a bad connection gets
 * rendered HTML rather than a spinner that resolves into a fetch waterfall.
 */
export const load: PageServerLoad = async ({ cookies, fetch, url }) => {
	const token = cookies.get(TOKEN_COOKIE);
	if (!token) redirect(303, '/sign-in');

	let shops;
	try {
		shops = await outlets(token, fetch);
	} catch {
		// A token that stopped working — revoked, or the server rebuilt. Clearing it sends them
		// back to sign in rather than to an error page they can do nothing about.
		cookies.delete(TOKEN_COOKIE, { path: '/' });
		redirect(303, '/sign-in');
	}

	const chosen = url.searchParams.get('outlet') ?? shops[0]?.id;
	if (!chosen) return { shops, chosen: null, today: null, error: null };

	try {
		return { shops, chosen, today: await day(token, chosen, {}, fetch), error: null };
	} catch (thrown) {
		// The list loaded, so the token is good — this is the report failing. Say so rather than
		// signing them out, which would look like the same problem as a bad token.
		return {
			shops,
			chosen,
			today: null,
			error: thrown instanceof Error ? thrown.message : 'the report could not be read'
		};
	}
};
