/**
 * How this outlet's numbers are written.
 *
 * Every screen used to build its own formatters from the literals `BDT` and `Asia/Dhaka`, which
 * meant a Riyadh café rendered riyals with a taka sign and parsed entry against the wrong minor
 * unit. One shop, one answer — read from the outlet the till is actually configured as.
 *
 * Held as module state rather than passed down: the currency is a property of the shop, not of any
 * screen, and threading it through every component would give each one a chance to disagree.
 */
import { createFormatters, type CurrencyCode, type Formatters } from '@sahl/ui';
import { till } from './till';

/** Until the outlet has been read, there is nothing to format. */
let currency = $state<CurrencyCode | null>(null);
let timeZone = $state<string | null>(null);

/**
 * Formatters for this outlet.
 *
 * Falls back to UTC and taka *only* before the outlet has been read — a screen rendering during
 * that first tick has no figures on it yet, because every figure comes from the till.
 */
export const shop = {
	get formatters(): Formatters {
		return createFormatters({
			locale: 'en',
			currency: currency ?? 'BDT',
			timeZone: timeZone ?? 'UTC'
		});
	},
	/** The currency to parse keyboard entry against. Null until the outlet is known. */
	get currency(): CurrencyCode | null {
		return currency;
	},
	get configured(): boolean {
		return currency !== null;
	}
};

/**
 * Read the outlet and remember how it writes numbers.
 *
 * Safe to call from any screen's mount: an unconfigured till simply leaves it unknown, and the
 * screens that need to sell will refuse for that reason anyway.
 */
export async function loadShop(): Promise<void> {
	try {
		const outlet = await till.outletConfig();
		if (!outlet) return;
		currency = outlet.currency as CurrencyCode;
		timeZone = outlet.timezone;
	} catch {
		// A till that cannot be read is a problem the calling screen will report; this only decides
		// how numbers look, and guessing loudly here would bury that message under a second one.
	}
}
