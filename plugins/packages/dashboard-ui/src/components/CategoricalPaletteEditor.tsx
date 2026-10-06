import React from "react";
import { Button, cn } from "@loams-plugins/core/ui";
import { ColorField } from "./ColorField";
import { HELP } from "./fields";

interface CategoricalPaletteEditorProps {
  colors: string[];
  onChange: (next: string[]) => void;
}

/* ------------------------------------------------------------- fragments */

/** `m-0` because preflight resets the default UA margin on `<fieldset>`. */
const FIELDSET =
  "m-0 flex flex-col gap-[0.8rem] rounded border border-line p-[0.85rem_0.9rem_0.95rem]";

const LEGEND = "px-[0.35rem] text-[0.72rem] font-bold tracking-[0.04em] text-ink-muted uppercase";

const LIST = "m-0 flex list-none flex-col gap-[0.6rem]";

const ITEM = "flex items-end gap-[0.55rem]";

const INDEX = "w-[18px] shrink-0 pb-[0.55rem] text-right text-[0.72rem] font-bold text-ink-muted";

const BODY = "min-w-0 flex-1";

/** Fixed 26px hit targets so the arrows line up under the swatch, not the label. */
const ACTIONS = "flex gap-1 pb-[0.1rem] [&_.min-w-7]:min-w-7";

const ACTION = "min-w-7 cursor-pointer px-1 disabled:cursor-not-allowed disabled:opacity-40";

/**
 * Editor for `series.categorical`.
 *
 * Order is semantically meaningful — flint assigns series to colours
 * positionally, so the second swatch is the second series in every chart. That
 * is why this offers reorder rather than treating the list as a set, and why
 * the copy says so out loud.
 *
 * Reordering is done with explicit buttons rather than drag-and-drop: keyboard
 * operable, and it needs no dependency.
 */
export const CategoricalPaletteEditor: React.FC<CategoricalPaletteEditorProps> = ({
  colors,
  onChange,
}) => {
  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= colors.length) return;
    const next = [...colors];
    const [item] = next.splice(index, 1);
    next.splice(target, 0, item);
    onChange(next);
  };

  const remove = (index: number) => {
    onChange(colors.filter((_, i) => i !== index));
  };

  const add = () => {
    // Distinct enough to be visible, and obviously a starting point to edit.
    onChange([...colors, "#7a5af8"]);
  };

  return (
    <fieldset className={FIELDSET}>
      <legend className={LEGEND}>Series colours (categorical)</legend>
      <p className={cn(HELP, "m-0")}>
        Order matters: the colour in position <em>n</em> is what the <em>n</em>th series wears in
        every chart. Reorder to remap a whole dashboard at once.
      </p>

      <ol className={LIST}>
        {colors.map((color, index) => (
          <li key={`${index}-${color}`} className={ITEM}>
            <span className={INDEX} aria-hidden="true">
              {index + 1}
            </span>
            <div className={BODY}>
              <ColorField
                id={`theme-cat-${index}`}
                label={`Series ${index + 1}`}
                value={color}
                onChange={(next) => onChange(colors.map((c, i) => (i === index ? next : c)))}
              />
            </div>
            <div className={ACTIONS}>
              <Button
                variant="outline"
                size="xs"
                className={ACTION}
                onClick={() => move(index, -1)}
                disabled={index === 0}
                aria-label={`Move series ${index + 1} colour earlier`}
              >
                ↑
              </Button>
              <Button
                variant="outline"
                size="xs"
                className={ACTION}
                onClick={() => move(index, 1)}
                disabled={index === colors.length - 1}
                aria-label={`Move series ${index + 1} colour later`}
              >
                ↓
              </Button>
              <Button
                variant="destructive-outline"
                size="xs"
                className={ACTION}
                onClick={() => remove(index)}
                aria-label={`Remove series ${index + 1} colour`}
              >
                ✕
              </Button>
            </div>
          </li>
        ))}
      </ol>

      <Button variant="outline" size="xs" onClick={add}>
        Add colour
      </Button>
    </fieldset>
  );
};
