/**
 * `@sahl/ui` — the shared design system.
 *
 * Consumers import `@sahl/ui/tokens.css` once at the app root (which pulls in Tailwind and the
 * bundled fonts), then these components. Nothing here computes money or tax: values arrive already
 * exact from `sahl-core` and already formatted by `createFormatters`.
 */

export { default as Badge } from './components/Badge.svelte';
export { default as Button } from './components/Button.svelte';
export { default as Card } from './components/Card.svelte';
export { default as Checkbox } from './components/Checkbox.svelte';
export { default as Field } from './components/Field.svelte';
export { default as Input } from './components/Input.svelte';
export { default as Logo } from './components/Logo.svelte';
export { default as Numeric } from './components/Numeric.svelte';
export { default as Select } from './components/Select.svelte';

export { createFormatters, minorToDecimalString, parseMinor, FormatError } from './lib/format.js';
export type { CurrencyCode, FormatContext, Formatters, Locale } from './lib/format.js';
