/**
 * Tests for the one place TypeScript touches money exactness.
 *
 * Everything else in the UI displays a number Rust computed. `parseMinor` is the opposite
 * direction — a keyboard entry becoming a value the till will act on — so it is the only function
 * here that can put a wrong number into the event log.
 */

import { describe, expect, it } from 'vitest';

import { createFormatters, minorToDecimalString, parseMinor } from './format.js';

describe('parseMinor', () => {
	it('reads whole and fractional amounts exactly', () => {
		expect(parseMinor('500', 'BDT')).toBe(50_000);
		expect(parseMinor('499.50', 'BDT')).toBe(49_950);
		expect(parseMinor('0.05', 'BDT')).toBe(5);
		expect(parseMinor('0', 'BDT')).toBe(0);
	});

	it('pads a short fraction rather than misreading it', () => {
		// "499.5" is four hundred ninety-nine and fifty paisa, not five.
		expect(parseMinor('499.5', 'BDT')).toBe(49_950);
	});

	it('accepts negatives, which cash movements need', () => {
		expect(parseMinor('-1000', 'BDT')).toBe(-100_000);
		expect(parseMinor('-0.01', 'BDT')).toBe(-1);
	});

	it('tolerates surrounding whitespace', () => {
		expect(parseMinor('  500  ', 'BDT')).toBe(50_000);
	});

	it('refuses anything malformed rather than guessing', () => {
		// A cash amount the UI misread is a drawer that will not balance.
		for (const entry of ['', ' ', 'abc', '5.', '.5', '1,000', '5.123', '1e3', '--5', '5-']) {
			expect(parseMinor(entry, 'BDT'), entry).toBeNull();
		}
	});

	it('refuses more fraction digits than the currency has', () => {
		expect(parseMinor('1.005', 'BDT')).toBeNull();
	});

	it('refuses a value too large to stay an exact integer', () => {
		// Past Number.MAX_SAFE_INTEGER, JS silently drops the last digit — the whole reason this
		// module never divides money.
		expect(parseMinor('99999999999999999', 'BDT')).toBeNull();
	});

	it('round-trips against minorToDecimalString', () => {
		for (const minor of [0, 1, 5, 99, 100, 49_950, -1, -100_000, 8_675_309]) {
			const decimal = minorToDecimalString(minor, 2);
			expect(parseMinor(decimal, 'BDT'), decimal).toBe(minor);
		}
	});

	it('rejects an unsupported currency loudly', () => {
		expect(() => parseMinor('500', 'XYZ' as never)).toThrow(/unsupported currency/);
	});
});

describe('createFormatters', () => {
	it('renders Western digits in every locale', () => {
		// bn-BD would otherwise render ১,২৩৪ and ar-SA ١٬٢٣٤ — neither of which Geist Mono has, and
		// neither of which ZATCA accepts on an invoice.
		for (const locale of ['en', 'bn-BD', 'ar-SA'] as const) {
			const format = createFormatters({ locale, currency: 'BDT', timeZone: 'Asia/Dhaka' });
			expect(format.moneyPlain(123_450), locale).toMatch(/[0-9]/);
			expect(format.moneyPlain(123_450), locale).not.toMatch(/[০-৯٠-٩]/);
		}
	});

	it('formats a value beyond float precision exactly', () => {
		const format = createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Asia/Dhaka' });
		expect(format.moneyPlain(9_007_199_254_740_993n)).toContain('90,071,992,547,409.93');
	});

	it('refuses a missing or unknown timezone', () => {
		// A POS reports by business day; falling back to the device clock mis-assigns evening sales.
		expect(() => createFormatters({ locale: 'en', currency: 'BDT', timeZone: '' })).toThrow(
			/timeZone is required/
		);
		expect(() =>
			createFormatters({ locale: 'en', currency: 'BDT', timeZone: 'Mars/Olympus' })
		).toThrow(/unknown IANA timezone/);
	});
});
