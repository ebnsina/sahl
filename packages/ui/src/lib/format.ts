/**
 * Formatting — `Intl` only, never a hand-rolled formatter.
 *
 * The division of labour is strict: **Rust computes, JavaScript formats.** Every value arriving
 * here is an exact integer produced by `sahl-core` — money in minor units, quantity in thousandths,
 * rates in basis points. Nothing in this file does arithmetic that could change a value; it only
 * decides how one is rendered.
 *
 * Two things here are load-bearing and easy to get wrong.
 *
 * **1. Western digits are forced everywhere.** `Intl.NumberFormat('bn-BD')` renders Bengali
 * numerals (১,২৩৪.৫০) and `('ar-SA')` renders Arabic-Indic (١٬٢٣٤٫٥٠) by default. Geist Mono has
 * neither glyph set, so numerals would silently fall back to some system font — destroying the
 * tabular alignment that is the entire reason for a mono numeric face — and ZATCA expects Western
 * digits on an invoice regardless. `numberingSystem: 'latn'` is therefore not optional.
 *
 * **2. Money is formatted from an exact decimal string, never a divided float.** `minor / 100`
 * introduces a float, and `i64` minor units can exceed `Number.MAX_SAFE_INTEGER`. `Intl.NumberFormat`
 * accepts a string and formats it exactly, so the integer is converted to a decimal string by digit
 * manipulation and handed over untouched.
 */

/** Locales the product ships in. */
export type Locale = 'en' | 'bn-BD' | 'ar-SA';

/** Currencies matching `sahl_core::money::Currency`. */
export type CurrencyCode = 'BDT' | 'SAR' | 'AED' | 'USD';

/**
 * Minor-unit exponents.
 *
 * **Must stay in sync with `Currency::exponent` in `crates/sahl-core/src/money/currency.rs`.**
 * A mismatch here shifts every price by a factor of ten, so adding a currency means changing both.
 */
const CURRENCY_EXPONENT: Record<CurrencyCode, number> = {
	BDT: 2,
	SAR: 2,
	AED: 2,
	USD: 2
};

/** Thousandths per unit, matching `Quantity::MILLI_PER_UNIT`. */
const QUANTITY_EXPONENT = 3;

/** Basis points per whole, matching `Rate::BASIS_POINTS_PER_UNIT`. */
const BASIS_POINTS_PER_UNIT = 10_000;

/** Forced on every numeric formatter. See the note at the top of this file. */
const NUMBERING_SYSTEM = 'latn' as const;

/**
 * `Intl.RelativeTimeFormat` accepts `numberingSystem` per ECMA-402, and honours it — verified:
 * `ar-SA` yields `قبل 3 دقائق` with it and `قبل ٣ دقائق` without. TypeScript's bundled lib types
 * simply omit the field, so it is declared here rather than dropped. Dropping it would put
 * Arabic-Indic digits back into the one place we most need Western ones.
 */
type RelativeTimeOptions = Intl.RelativeTimeFormatOptions & { numberingSystem?: string };

/**
 * `Intl.NumberFormat.prototype.format` accepts a decimal *string* and formats it exactly
 * (Intl.NumberFormat v3). TypeScript types the parameter as the template-literal type
 * `StringNumericLiteral`, which no dynamically built string can satisfy statically.
 *
 * The cast is safe because `minorToDecimalString` is the only producer and emits `-?\d+(\.\d+)?`.
 * Passing a real `number` instead would reintroduce the float division this module exists to avoid.
 */
function exact(decimal: string): Intl.StringNumericLiteral {
	return decimal as Intl.StringNumericLiteral;
}

export class FormatError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'FormatError';
	}
}

/**
 * Render an exact integer of minor units as a decimal string.
 *
 * Pure digit manipulation — no division, so no float ever exists and arbitrarily large `i64` values
 * survive intact.
 */
export function minorToDecimalString(minor: bigint | number, exponent: number): string {
	if (exponent < 0 || !Number.isInteger(exponent)) {
		throw new FormatError(`exponent must be a non-negative integer, received ${exponent}`);
	}
	const value = typeof minor === 'bigint' ? minor : BigInt(minor);
	const negative = value < 0n;
	const digits = (negative ? -value : value).toString().padStart(exponent + 1, '0');
	const splitAt = digits.length - exponent;
	const whole = digits.slice(0, splitAt);
	const fraction = digits.slice(splitAt);
	const sign = negative ? '-' : '';
	return exponent === 0 ? `${sign}${whole}` : `${sign}${whole}.${fraction}`;
}

/**
 * A set of memoised formatters bound to one locale, currency, and timezone.
 *
 * Constructing an `Intl` formatter is expensive and a sell screen re-renders on every keystroke, so
 * they are built once per context rather than per call.
 *
 * Timezone is **required**, deliberately. A POS reports by business day, and falling back to the
 * device's system timezone would silently mis-assign late-evening sales to the wrong day — the kind
 * of bug that surfaces as an unexplained gap in a monthly report months later.
 */
export interface FormatContext {
	locale: Locale;
	currency: CurrencyCode;
	/** IANA timezone of the outlet, e.g. `Asia/Dhaka` or `Asia/Riyadh`. */
	timeZone: string;
}

export interface Formatters {
	/** Money from exact minor units, with currency symbol. */
	money(minor: bigint | number): string;
	/** Money from exact minor units, digits only — for table columns with a currency header. */
	moneyPlain(minor: bigint | number): string;
	/** Quantity from exact thousandths, trailing zeros trimmed. */
	quantity(milli: bigint | number): string;
	/** A rate from basis points: `1500` renders as `15%`. */
	percent(basisPoints: number): string;
	/** A plain whole number, grouped. */
	integer(value: bigint | number): string;
	/** Date and time in the outlet's timezone. */
	dateTime(millis: number): string;
	/** Date only, in the outlet's timezone. */
	date(millis: number): string;
	/** Time only, in the outlet's timezone. */
	time(millis: number): string;
	/** Relative time — "3 minutes ago" — for sync status and activity feeds. */
	relative(millis: number, now?: number): string;
}

function assertCurrency(currency: string): asserts currency is CurrencyCode {
	if (!(currency in CURRENCY_EXPONENT)) {
		throw new FormatError(
			`unsupported currency ${currency}; add it here and to Currency in sahl-core`
		);
	}
}

function assertTimeZone(timeZone: string): void {
	if (!timeZone) {
		throw new FormatError('timeZone is required — a POS reports by the outlet business day');
	}
	try {
		new Intl.DateTimeFormat('en', { timeZone });
	} catch {
		throw new FormatError(`unknown IANA timezone: ${timeZone}`);
	}
}

/**
 * Parse a typed decimal string into exact minor units.
 *
 * The inverse of `minorToDecimalString`, and the only sanctioned way a keyboard entry becomes a
 * number the till will act on. Returns `null` on anything malformed rather than a best guess — a
 * cash amount the UI misread is a drawer that will not balance, and refusing is recoverable in a
 * way a silent misparse is not.
 *
 * Digit manipulation, no float: `"499.5"` with exponent 2 becomes `49950`, not `499.5 * 100`.
 */
export function parseMinor(entry: string, currency: CurrencyCode): number | null {
	assertCurrency(currency);
	const exponent = CURRENCY_EXPONENT[currency];
	const trimmed = entry.trim();

	// `\d{1,n}` after the point, not `\d{0,n}`: a bare trailing dot is a half-typed entry, and
	// reading "5." as five is a guess about what someone was about to type.
	const pattern = new RegExp(`^-?\\d+(\\.\\d{1,${exponent}})?$`);
	if (!pattern.test(trimmed)) return null;

	const negative = trimmed.startsWith('-');
	const [whole = '0', fraction = ''] = (negative ? trimmed.slice(1) : trimmed).split('.');
	const digits = `${whole}${fraction.padEnd(exponent, '0')}`;

	const value = Number(digits);
	// Beyond this, integer arithmetic in JS stops being exact — and a money value that silently
	// loses its last digit is precisely what this module exists to prevent.
	if (!Number.isSafeInteger(value)) return null;
	return negative ? -value : value;
}

/**
 * Build the formatters for a given outlet context.
 *
 * Validates eagerly and throws on bad configuration rather than rendering something plausible but
 * wrong — the same fail-fast posture the server takes with environment variables.
 */
export function createFormatters(context: FormatContext): Formatters {
	const { locale, currency, timeZone } = context;
	assertCurrency(currency);
	assertTimeZone(timeZone);

	const exponent = CURRENCY_EXPONENT[currency];

	const moneyFormat = new Intl.NumberFormat(locale, {
		style: 'currency',
		currency,
		// `narrowSymbol` prefers the currency's own sign over its ISO code: ৳1,234.50 rather than
		// BDT 1,234.50. A merchant reads their own currency sign, not a three-letter code.
		//
		// Where CLDR has no narrow symbol for a currency in a given locale, Intl falls back to the
		// code on its own — SAR and AED do this in English, though both render their native symbol
		// (ر.س. / د.إ.) in Arabic locales, which is where they are actually sold. Left to Intl rather
		// than overridden with a hand-picked glyph, since a hardcoded symbol is a formatting rule in
		// disguise and would drift from CLDR.
		currencyDisplay: 'narrowSymbol',
		numberingSystem: NUMBERING_SYSTEM,
		minimumFractionDigits: exponent,
		maximumFractionDigits: exponent
	});

	const moneyPlainFormat = new Intl.NumberFormat(locale, {
		numberingSystem: NUMBERING_SYSTEM,
		minimumFractionDigits: exponent,
		maximumFractionDigits: exponent
	});

	const quantityFormat = new Intl.NumberFormat(locale, {
		numberingSystem: NUMBERING_SYSTEM,
		minimumFractionDigits: 0,
		maximumFractionDigits: QUANTITY_EXPONENT
	});

	const percentFormat = new Intl.NumberFormat(locale, {
		style: 'percent',
		numberingSystem: NUMBERING_SYSTEM,
		minimumFractionDigits: 0,
		maximumFractionDigits: 2
	});

	const integerFormat = new Intl.NumberFormat(locale, {
		numberingSystem: NUMBERING_SYSTEM,
		maximumFractionDigits: 0
	});

	const dateTimeFormat = new Intl.DateTimeFormat(locale, {
		timeZone,
		numberingSystem: NUMBERING_SYSTEM,
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	const dateFormat = new Intl.DateTimeFormat(locale, {
		timeZone,
		numberingSystem: NUMBERING_SYSTEM,
		dateStyle: 'medium'
	});

	const timeFormat = new Intl.DateTimeFormat(locale, {
		timeZone,
		numberingSystem: NUMBERING_SYSTEM,
		timeStyle: 'short'
	});

	// Declared as a typed variable rather than an inline literal: TypeScript's excess-property check
	// fires on literals passed directly as arguments, and its bundled `RelativeTimeFormatOptions`
	// omits `numberingSystem` even though ECMA-402 specifies it.
	const relativeOptions: RelativeTimeOptions = {
		numberingSystem: NUMBERING_SYSTEM,
		numeric: 'auto'
	};
	const relativeFormat = new Intl.RelativeTimeFormat(locale, relativeOptions);

	return {
		money: (minor) => moneyFormat.format(exact(minorToDecimalString(minor, exponent))),
		moneyPlain: (minor) => moneyPlainFormat.format(exact(minorToDecimalString(minor, exponent))),
		quantity: (milli) =>
			quantityFormat.format(exact(minorToDecimalString(milli, QUANTITY_EXPONENT))),
		// `style: 'percent'` multiplies by 100, so basis points divide by 10,000 to get the fraction.
		// This is display-only arithmetic on a bounded small integer — never on a money value.
		percent: (basisPoints) => percentFormat.format(basisPoints / BASIS_POINTS_PER_UNIT),
		integer: (value) => integerFormat.format(value),
		dateTime: (millis) => dateTimeFormat.format(new Date(millis)),
		date: (millis) => dateFormat.format(new Date(millis)),
		time: (millis) => timeFormat.format(new Date(millis)),
		relative: (millis, now = Date.now()) => {
			// Walk up the units until the value fits, so "45 seconds ago" and "2 months ago" both
			// read naturally. The final entry has an infinite limit, so the loop always returns.
			const scale: Array<[Intl.RelativeTimeFormatUnit, number]> = [
				['second', 60],
				['minute', 60],
				['hour', 24],
				['day', 7],
				['week', 4.348],
				['month', 12],
				['year', Number.POSITIVE_INFINITY]
			];

			let value = (millis - now) / 1000;
			for (const [unit, limit] of scale) {
				if (Math.abs(value) < limit) {
					return relativeFormat.format(Math.round(value), unit);
				}
				value = value / limit;
			}
			return relativeFormat.format(Math.round(value), 'year');
		}
	};
}
