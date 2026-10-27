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
<html class="dark"></html>
```

Light mode is a warm window with the work area as a white card; dark mode is
a neutral-black canvas (design 59, D1073). Indigo is the one accent; emerald,
amber, red and blue mean state only. The class also flips `color-scheme`, so form controls and
scrollbars follow. `onThemeChange(fn)` calls `fn` whenever the class or the
inline style of `<html>` changes, and returns a function that disconnects the
observer.

## Fonts

The tokens use the system UI and monospace stacks (design 59, D1075); no font
is bundled. Monospace is for code, ids and SQL only.

## Tokens

Every colour, radius, shadow, size and duration is a `--op-*` variable in
`tokens.css`, in both themes; the console's Tailwind theme maps each to a
utility (`bg-elevated`, `border-control-border`, `outline-focus`, `rounded-lg`,
`text-body`, `shadow-toast`, `ease-out`, …). Use the role, not the value:

| Role           | Tokens (utilities)                                                                                                                                                                      |
| -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Canvas, window | `--op-bg` (`bg-bg`), `--op-window` (`bg-window`)                                                                                                                                        |
| Surfaces       | `--op-surface` (cards, panels), `--op-elevated` (menus, popovers, dialogs, toasts), `--op-raised` (muted fill, hover)                                                                   |
| Text           | `--op-ink`, `--op-muted`, `--op-faint`; `--op-accent-fg` for the accent as text                                                                                                         |
| Primary action | `--op-accent`, `--op-accent-hover`, `--op-accent-active`, `--op-accent-ink` (its label), `--op-accent-soft`                                                                             |
| State          | `--op-grow` (success), `--op-warn`, `--op-danger` (+ `-hover` for the solid button), `--op-info`, each with `-soft`                                                                     |
| Borders        | `--op-rule` (border), `--op-rule-soft` (separator), `--op-input` (field outline, 3:1), `--op-control-border` (button outline)                                                           |
| Focus          | `--op-focus`, `--op-focus-width` (2px), `--op-focus-offset` (2px; rows inset it)                                                                                                        |
| Rows           | `--op-row-hover`, `--op-row-selected`, `--op-shadow-chip`; heights `--op-row` (28) and `--op-row-comfortable` (32)                                                                      |
| Controls       | `--op-control-xs` 24, `--op-control-compact` 28, `--op-control` 32, `--op-control-input` 36; icons `--op-icon-xs` 12, `--op-icon-sm` 14, `--op-icon` 16                                 |
| Type           | `--op-text-micro` 11 … `--op-text-hero` 24; `--op-weight-regular`/`-medium`/`-semibold`                                                                                                 |
| Radii          | `--op-radius-xs` 4, `-sm` 6 (rows, chips), `-md` 8 (controls), `-lg` 14 (cards), `-xl` 22 (dialogs), `-pill`                                                                            |
| Shadows        | only for what floats: `--op-shadow-pop`, `-modal`, `-toast`, `-composer`, `-chip`                                                                                                       |
| Motion         | `--op-duration-fast` 120 (hover), `--op-duration-enter` 160, `--op-duration-exit` 110, `--op-duration-page` 220; `--op-ease-out`, `--op-ease-in`, `--op-ease-in-out`, `--op-ease-panel` |

`prefers-reduced-motion: reduce` sets every duration to 0ms. Panels still
snap unless Settings › Appearance › Motion is on (`--op-panel-duration`).

## Motion

Motion for React (`motion`) is the one animation library, and `@loams/ui`
re-exports what plugins need (`motion`, `AnimatePresence`), so nothing else
depends on it directly. The rules: transform and opacity only; nothing moves
without a state change; nothing waits for an animation before it can be used;
reduced motion is an instant change.

- **Base UI primitives animate in CSS.** Menus, popovers, selects, tooltips,
  dialogs, sheets, the palette and toasts use Base UI's
  `data-starting-style`/`data-ending-style` with the duration tokens: in at
  160ms on `ease-out` from the trigger (`origin-(--transform-origin)`), out at
  110ms on `ease-in`. Base UI waits for the transition before it unmounts, so
  focus return and dismissal stay correct. Use the primitives and you get it.
- **Presets** (`MOTION_PRESETS`: `fade`, `fadeUp`, `scaleIn`, `toast`) for
  anything Motion renders: `const p = useMotionPreset('fadeUp'); <motion.div {...p} />`.
  `useMotionPreset` returns `REDUCED_PRESET` under reduced motion;
  `motionPreset(name, reduced)` is the pure form, `useMotionTransition(t)` the
  same for a single transition. `MOTION_DURATION`, `MOTION_EASE` and
  `MOTION_SPRING` hold the numbers (seconds), equal to the CSS tokens.
- **A new overlay primitive built with Motion** (inside `@loams/ui`, the only
  package that imports Base UI): control `open`, wrap the portal in
  `OverlayPresence`, keep it mounted, and render the popup through Motion. Do
  not add `OVERLAY_SURFACE`'s transition classes to it. Plugins use the
  primitives, which already animate.

  ```tsx
  const [open, setOpen] = useState(false);
  const popup = useMotionRender('scaleIn');
  <PopoverRoot open={open} onOpenChange={setOpen}>
    <PopoverTrigger>Filters</PopoverTrigger>
    <OverlayPresence open={open}>
      <Popover.Portal keepMounted>
        <Popover.Positioner>
          <Popover.Popup render={popup}>…</Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </OverlayPresence>
  </PopoverRoot>;
  ```

- **Skeleton to content:** `<LoadingSwap loading={q.isPending} skeleton={<SkeletonList … />}>…</LoadingSwap>`
  crossfades once loading ends; the first render does not animate.
- **Lists:** `<StaggerList>` with `<StaggerItem index={i}>` rises the first
  eight items in order (25ms apart) when the list appears.
- **Selection:** `<SelectionIndicator layoutId="…" />` inside the selected
  option slides to the next one on a spring; wrap the options in
  `<SelectionGroup>` so two groups on a page stay apart. `Tabs` and
  `Segmented` use it.
- `MotionProvider` sets `reducedMotion="user"` and the default transition for
  a tree; the helpers above work without it.

## States

Every interactive primitive has default, hover, pressed, focus-visible (the
2px accent ring, offset 2px; inset on rows) and disabled states, and these:

- `Button`: `loading` keeps the label and width, shows a spinner, sets
  `aria-busy` and `aria-disabled` (it stays focusable) and refuses presses.
  `IconButton` takes a required `label` (its name and tooltip) and `size`
  `md`/`sm`/`xs`.
- Fields (`InputControl`, `TextareaControl`, `SearchInput`, `SelectTrigger`):
  hover darkens the outline; `aria-invalid` paints it danger (and the focus
  ring); disabled dims and shows `not-allowed`. `SearchInput` adds a search
  icon, a clear button and Escape-to-clear.
- `ListRow`: hover, `selected` (or `aria-current="page"`/`aria-selected`),
  `disabled`, `density` `compact` (28px) or `default` (32px); render it as the
  link or button it is.

## Exports

Primitives

- `Button`, `ButtonProps`, `ButtonSize`, `ButtonVariant` — the one action
  control; `buttonClass` builds its class name outside React. `IconButton`,
  `IconButtonProps` — a labelled icon-only button.
- `SearchInput` — a search field with a clear button; `ListRow` — one row in
  a list or a sidebar.
- `LoadingSwap`, `StaggerList`, `StaggerItem`, `SelectionIndicator`,
  `SelectionGroup`, `OverlayPresence`, `useMotionPreset`, `useMotionRender`,
  `MotionProvider`, `motion`, `AnimatePresence` — the motion system (above).
- `Card` — a bordered, rounded surface with an optional title.
- `Empty` — the "nothing here yet" state.
- `Field`, `Input`, `Textarea`, `Select`, `Checkbox` — labelled form controls
  that share one focus ring and error styling.
- `Logo`, `Mark`, `Wordmark` — the Loams mark, the bare glyph and the
  glyph-with-name lockup.
- `Avatar`, `Meter`, `Notice`, `NoticeTone`, `Snippet`, `Stat`, `Stats` — the
  remaining console pieces.
- `Badge`, `Status`, `StatusTag` — status pills; `Status` is the state a pill
  renders.
- `Table`, `Column` — a data table; a `Column` names its `key`, its `header`,
  a `cell` renderer, and optionally `numeric` (right-aligned, tabular figures)
  and `width`.
- `DataTable`, `SortableColumn` — `Table` with sortable columns (TanStack
  Table); `VirtualList` — a long list that renders only what is in view
  (TanStack Virtual).
- `Grain`, `GrainKind` — the texture overlays.
- `cx` — joins class names, dropping falsy ones.

Overlays and composite controls (Base UI, styled with Tailwind token utilities;
see [docs/design/console-ui-stack.md](../../../docs/design/console-ui-stack.md))

- `Dialog` — a modal: focus moves in and is trapped, Escape and the scrim
  close it unless `dismissible` is false.
- `Menu`, `MenuItem`, `MenuLabel`, `MenuSeparator` — an action menu on a trigger.
- `Popover` — non-modal content anchored to a trigger.
- `SelectMenu`, `Option` — a styled select; `Combobox` — a filtering text input.
- `Tabs`, `Tab` — tabs with arrow-key movement, with or without panels.
- `Tooltip`, `TooltipProvider` — a short hover or focus label.
- `ToastProvider`, `Toaster`, `useToast` — transient messages.
- `AlertDialog` — a confirmation (role `alertdialog`) with a confirm and a
  cancel button; `Sheet` — a modal panel from the left, right or bottom edge.

Plugins import these from `@loams/ui`, never from `@base-ui/react` directly.

Studio's primitives, adapted from T3 Code's `components/ui` (MIT; see
`NOTICE`, `LICENSE.t3code` and `provenance.json`)

- `CommandPalette`, `CommandItem`, `matchCommand`, `Shortcut` — the palette:
  a dialog around an always-open autocomplete; arrows move, Enter runs, long
  lists are virtualised.
- `Sidebar` and `SidebarHeader`, `SidebarContent`, `SidebarFooter`,
  `SidebarGroup`, `SidebarMenu`, `SidebarMenuItem`, `SidebarMenuButton`,
  `SidebarMenuSub` — a resizable sidebar and its rows; `ResizeHandle` — the
  keyboard-operable splitter it uses, for panels and drawers too.
- `Kbd`, `KbdGroup` — keys and chords.
- `Toggle`, `ToggleGroup` — toggle buttons, one or several pressed
  (`Segmented` stays the exactly-one pill row).
- `InputGroup`, `InputGroupAddon`, `InputGroupInput` — an input with a glyph,
  a hint or a button attached.
- `MiddleTruncate`, `splitForMiddleTruncate` — an ellipsis in the middle, for
  branches and paths.
- `ScrollArea` — thin overlay scrollbars and edge fades.
- `Switch` — an on/off setting that applies at once.
- `Collapsible`, `CollapsibleTrigger`, `CollapsiblePanel` — a disclosure.
- `Spinner`, `Skeleton` — loading states.

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
- `rgba` — turns a `#rgb` or `#rrggbb` token into `rgba(…, a)` (any other CSS
  colour through `color-mix()`), so a token read from the DOM can be used at a
  chosen alpha.
- `rng` — a seeded generator, so a grain texture is the same on every render.
- `onThemeChange`, `prefersReducedMotion` — the two environment checks the
  components make.

## Develop

See [`web/README.md`](../../README.md) for the workspace's install, lint,
typecheck and build commands.
