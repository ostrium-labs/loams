/*
 * Conditional class names with Tailwind conflict resolution.
 *
 * Reimplemented rather than copied from t3code: their `lib/utils.ts` pulls
 * `MessageId`/`ProjectId`/`ThreadId` from `@t3tools/contracts` and
 * `effect/Encoding` alongside the two-line `cn`, which would drag Effect into
 * this bundle for nothing. `clsx` and `tailwind-merge` are all it actually
 * needs.
 */
import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

/*
 * The theme's extra font sizes. Unregistered, tailwind-merge reads `text-2xs`
 * as a *colour* and drops it the moment a `text-muted-foreground` follows, so
 * the copy in `app.css` has to be declared here too or it silently loses.
 */
const twMerge = extendTailwindMerge({
  extend: { theme: { text: ["2xs", "3xs", "4xs", "5xs"] } },
});

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

export type { ClassValue };
