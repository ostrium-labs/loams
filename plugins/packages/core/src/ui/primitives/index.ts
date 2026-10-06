/*
 * The ported t3code primitives.
 *
 * A barrel so a component reaches for `./primitives/index.js` rather than
 * remembering which of the nine files a name happens to live in. Each file is
 * a verbatim t3code copy with its MIT header intact; see the header inside any
 * one of them.
 */
export { Alert, AlertAction, AlertDescription, AlertTitle } from "./alert.js";
export { Badge, badgeVariants } from "./badge.js";
export { Button, buttonVariants, InlineButton } from "./button.js";
export type { ButtonSize, ButtonVariant } from "./button.js";
export {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "./empty.js";
export { Kbd, KbdGroup } from "./kbd.js";
export { Label } from "./label.js";
export { Separator } from "./separator.js";
export { Skeleton } from "./skeleton.js";
export { Switch } from "./switch.js";
