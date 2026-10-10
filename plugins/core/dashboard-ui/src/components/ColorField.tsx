import React from "react";
import { InlineButton, Label, cn } from "@loams-core/host/ui";
import { CONTROL, CONTROL_MONO, GROUP, HELP, INVALID, LABEL } from "./fields";
import { isHexColor, readableInkOn, toHexColor } from "../theme/ink";

interface ColorFieldProps {
  id: string;
  label: string;
  /** Current value, or undefined when the theme leaves this field to a preset. */
  value?: string;
  onChange: (next: string) => void;
  /** Clears the override so the inherited preset value shows through again. */
  onClear?: () => void;
  hint?: string;
}

/* ------------------------------------------------------------- fragments */

/** Label on the left, reset link on the right, sharing a baseline. */
const HEAD = "flex items-baseline justify-between gap-2";

const ROW = "flex items-center gap-2";

/**
 * The native colour input.
 *
 * The `::-webkit`/`::-moz` swatch padding is what removes the platform's inner
 * border so the swatch fills the box; without it the picker renders as a 30px
 * box inside a 30px box.
 */
const SWATCH =
  "h-[30px] w-[30px] shrink-0 cursor-pointer rounded border border-line bg-transparent p-0 [&::-moz-color-swatch]:rounded-[2px] [&::-moz-color-swatch]:border-none [&::-webkit-color-swatch]:rounded-[2px] [&::-webkit-color-swatch]:border-none [&::-webkit-color-swatch-wrapper]:p-0.5";

const RESET = "p-0 text-[0.7rem] text-primary-ink hover:text-ink";

const CHIP = "inline-flex items-center gap-[0.35rem] text-[0.72rem]";

const CHIP_SWATCH = "inline-block size-4 rounded-[3px] border border-line-subtle";

const CHIP_LABEL = "text-ink-muted";

const CHIP_HEX = "font-[Cascadia_Mono,ui-monospace,SFMono-Regular,Menlo,monospace] text-ink-body";

/**
 * One ink field: a native colour input paired with a text field for the hex.
 *
 * The swatch is never the sole carrier of meaning — the hex text sits beside it
 * and is the authoritative readout, so a screen-reader user and anyone picking
 * an exact brand colour get the same information. `<input type="color">` is
 * native, so no picker dependency is added.
 *
 * The text field holds its own draft while the user types, because a partial
 * value like `#1a1` is not a valid colour and coercing it on every keystroke
 * makes precise entry impossible. The draft commits on blur or Enter.
 */
export const ColorField: React.FC<ColorFieldProps> = ({
  id,
  label,
  value,
  onChange,
  onClear,
  hint,
}) => {
  const normalized = value ? toHexColor(value) : null;
  // A swatch needs a colour even before the field is set, or the native input
  // renders as black and reads as "this theme is black".
  const swatchColor = normalized ?? "#ffffff";

  const commit = (raw: string) => {
    const hex = toHexColor(raw);
    if (hex) onChange(hex);
  };

  return (
    <div className={GROUP}>
      <div className={HEAD}>
        <Label className={LABEL} htmlFor={id}>
          {label}
        </Label>
        {onClear && (
          <InlineButton
            tone="muted"
            className={RESET}
            onClick={onClear}
            aria-label={`Reset ${label} to the inherited value`}
          >
            Reset
          </InlineButton>
        )}
      </div>
      <div className={ROW}>
        <input
          type="color"
          className={SWATCH}
          value={swatchColor}
          aria-label={`${label} colour picker`}
          onChange={(e) => {
            const hex = toHexColor(e.target.value);
            if (hex) onChange(hex);
          }}
        />
        <TextHexInput id={id} value={value} onCommit={commit} aria-label={`${label} hex value`} />
      </div>
      {hint && <span className={HELP}>{hint}</span>}
    </div>
  );
};

interface TextHexInputProps {
  id: string;
  value?: string;
  onCommit: (raw: string) => void;
  "aria-label": string;
}

const TextHexInput: React.FC<TextHexInputProps> = ({ id, value, onCommit, ...rest }) => {
  const [draft, setDraft] = React.useState(value ?? "");
  const [invalid, setInvalid] = React.useState(false);

  // Re-sync when the value changes from outside (a preset swap, a reset).
  React.useEffect(() => {
    setDraft(value ?? "");
    setInvalid(false);
  }, [value]);

  return (
    <input
      id={id}
      type="text"
      inputMode="text"
      spellCheck={false}
      autoComplete="off"
      className={cn(CONTROL, CONTROL_MONO, "min-w-0 flex-1", invalid && INVALID)}
      value={draft}
      placeholder="inherited"
      aria-invalid={invalid}
      aria-label={rest["aria-label"]}
      onChange={(e) => {
        setDraft(e.target.value);
        setInvalid(e.target.value.trim().length > 0 && !isHexColor(e.target.value));
      }}
      onBlur={() => {
        const hex = toHexColor(draft);
        if (hex) {
          onCommit(hex);
          setDraft(hex);
          setInvalid(false);
        } else if (draft.trim().length > 0) {
          setInvalid(true);
        }
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          const hex = toHexColor(draft);
          if (hex) {
            onCommit(hex);
            setDraft(hex);
            setInvalid(false);
          } else {
            setInvalid(true);
          }
        }
      }}
    />
  );
};

/** A read-only swatch with its hex alongside — used for read-only preview rows. */
export const ColorChip: React.FC<{ color: string; label?: string }> = ({ color, label }) => (
  <span className={CHIP} title={color}>
    <span
      className={CHIP_SWATCH}
      style={{ background: color, color: readableInkOn(color) }}
      aria-hidden="true"
    />
    {label && <span className={CHIP_LABEL}>{label}</span>}
    <code className={CHIP_HEX}>{color}</code>
  </span>
);
