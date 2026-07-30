/**
 * Reading the shop from the server.
 *
 * Every call happens on the SvelteKit server, never in the browser: the access token lives in an
 * httpOnly cookie so that a script on the page — injected, or from a dependency nobody audited —
 * cannot read it and start pulling a merchant's takings.
 *
 * Nothing here computes anything. The figures arrive totalled by `sahl_core::report`, which is the
 * same crate the till sells with, so the dashboard and the counter cannot disagree.
 */

/** The cookie the token lives in. */
export const TOKEN_COOKIE = 'sahl_dashboard_token';

export interface Money {
	currency: string;
	minor: number;
}

export interface Day {
	currency: string;
	sales: number;
	takings: Money;
	net: Money;
	tax: Money;
	discount: Money;
	average_sale: Money;
	voids: number;
	by_cashier: Array<{
		staff_id: string;
		sales: number;
		takings: Money;
		discount: Money;
		voids: number;
	}>;
	by_payment: Array<{ method: unknown; count: number; taken: Money }>;
	by_product: Array<{
		product_id: string;
		name: string;
		quantity_milli: number;
		revenue: Money;
	}>;
}

/** A day, and who the ids in it refer to. */
export interface DayReport {
	day: Day;
	staff: Array<{ id: string; name: string }>;
}

export interface Outlet {
	id: string;
	name: string;
}

/** Where the server is. Fails loudly rather than guessing — a wrong base URL is a silent 404 wall. */
function base(): string {
	const url = process.env.SAHL_SERVER_URL;
	if (!url) {
		throw new Error('SAHL_SERVER_URL is not set — the dashboard has no server to read from');
	}
	return url.replace(/\/$/, '');
}

async function read<T>(path: string, token: string, fetcher: typeof fetch): Promise<T> {
	const response = await fetcher(`${base()}${path}`, {
		headers: { authorization: `Bearer ${token}` }
	});

	if (response.status === 401) throw new Error('unauthorised');
	if (!response.ok) throw new Error(`the server said ${response.status}`);
	return (await response.json()) as T;
}

export function outlets(token: string, fetcher: typeof fetch): Promise<Outlet[]> {
	return read<Outlet[]>('/api/outlets', token, fetcher);
}

export function day(
	token: string,
	outlet: string,
	range: { from?: number; to?: number },
	fetcher: typeof fetch
): Promise<DayReport> {
	const query = new URLSearchParams({ outlet });
	if (range.from !== undefined) query.set('from', String(range.from));
	if (range.to !== undefined) query.set('to', String(range.to));
	return read<DayReport>(`/api/report/day?${query.toString()}`, token, fetcher);
}
