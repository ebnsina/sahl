import { fail, redirect } from '@sveltejs/kit';
import { TOKEN_COOKIE, outlets } from '$lib/api';
import type { Actions } from './$types';

export const actions: Actions = {
	/**
	 * Take an access token and, if the server accepts it, keep it.
	 *
	 * Verified before it is stored rather than after: a cookie holding a token that does not work
	 * sends somebody to a broken dashboard with no way to tell whether they mistyped or the token
	 * was revoked.
	 */
	default: async ({ request, cookies, fetch }) => {
		const form = await request.formData();
		const token = String(form.get('token') ?? '').trim();
		if (!token) return fail(400, { message: 'Paste the access token' });

		try {
			await outlets(token, fetch);
		} catch {
			// One message for every failure. Distinguishing "no such token" from "revoked" would
			// tell somebody working through guesses when they had guessed a real one.
			return fail(401, { message: 'That token was not accepted' });
		}

		cookies.set(TOKEN_COOKIE, token, {
			path: '/',
			httpOnly: true,
			sameSite: 'lax',
			// Only over TLS in production. Left off in development so the dashboard works on
			// localhost without a certificate.
			secure: process.env.NODE_ENV === 'production',
			maxAge: 60 * 60 * 24 * 30
		});

		redirect(303, '/');
	}
};
