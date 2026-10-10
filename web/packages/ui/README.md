![Loams — Your data. Your bucket.](../../../docs/assets/loams-banner.svg)

# `@loams/ui`

Loams’ design system: the tokens, the mark, the grain textures and the React
primitives the console and the plugins are built from.

## Use it

```bash
import '@loams/ui/styles.css'
import { Button, Card, Logo } from '@loams/ui'
```

`styles.css` carries the tokens and the primitives' own rules. `tokens.css` is
importable on its own (`@loams/ui/tokens.css`) when you want the variables
without the component styles.

Inside this workspace the package resolves to its TypeScript sources. Outside
it, the published package serves the built `dist` (`publishConfig`).

## Dark mode

Put the class `dark` on `<html>`:

```html
<html class="dark">
```

Light mode stands on white; dark mode is humus with chalk informational and
oxide failed. The class also flips `color-scheme`, so form controls and
scrollbars follow. `onThemeChange(fn)` calls `fn` whenever the class or the
inline style of `<html>` changes, and returns a function that disconnects the
observer.

## Fonts

Set `--loams-font-sans` and `--loams-font-mono` to your loaded faces; the
defaults are Archivo and Martian Mono with system fallbacks. The console
bundles both from Fontsource under the SIL Open Font License 1.1 (see
[`NOTICE`](../../../NOTICE)).

## Exports

Primitives

- `Button`, `ButtonProps`, `ButtonSize`, `ButtonVariant` — the one action
  control; `buttonClass` builds its class name outside React.
- `Card` — a bordered surface with an optional title.
- `Empty` — the "nothing here yet" state.
- `Field`, `Input`, `Textarea`, `Select`, `Checkbox` — labelled form controls
  that share one focus ring and error styling.
- `Logo`, `Mark`, `Wordmark` — the Loams mark, the bare glyph and the
  glyph-with-name lockup.
- `Avatar`, `Dialog`, `Meter`, `Notice`, `NoticeTone`, `Snippet`, `Stat`,
  `Stats` — the remaining console pieces.
- `Badge`, `Status`, `StatusTag` — status pills; `Status` is the state a pill
  renders.
- `Table`, `Column` — a data table; a `Column` names its `key`, its `header`,
  a `cell` renderer, and optionally `numeric` (right-aligned, tabular figures)
  and `width`.
- `Grain`, `GrainKind` — the texture overlays.
- `cx` — joins class names, dropping falsy ones.

Formatters

- `formatBytes`, `formatNumber`, `formatDate`, `formatDuration`,
  `formatRelative` — the console's number and time conventions, in one place so
  two views never disagree.

Canvas helpers

- `Tokens`, `readTokens` — the resolved `bg`, `ink`, `accent` and `grow`
  colours, read from `--op-*` on `:root`, for anything drawn on a canvas where
  CSS custom properties are not available.
- `fit` — sizes a canvas to its box at the device pixel ratio (capped at 2x)
  and returns its 2d context, or `null` when the browser has none.
- `rgba` — turns a `#rgb` or `#rrggbb` string into `rgba(…, a)`, so a token
  read from the DOM can be used at a chosen alpha.
- `rng` — a seeded generator, so a grain texture is the same on every render.
- `onThemeChange`, `prefersReducedMotion` — the two environment checks the
  components make.

## Develop

See [`web/README.md`](../../README.md) for the workspace's install, lint,
typecheck and build commands.
