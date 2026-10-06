/**
 * The Flint ink -> CSS variable projection.
 *
 * This is the seam the whole shell hangs off: `useTheme` writes whatever
 * `inkToCssVars` returns onto `<html>`, and every themed surface -- the header,
 * the inspector, the widget cards -- reads those variables back. Nothing else
 * in the workspace tests it, so a rename or a dropped field here would fail
 * silently: surfaces would simply fall back to their stylesheet defaults and
 * the theme would appear to do nothing.
 *
 * It is a pure function over plain data, so it needs no DOM.
 */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import {
  THEME_CSS_VARS,
  categoricalSwatches,
  describeSelection,
  inkToCssVars,
  isHexColor,
  readableInkOn,
  toHexColor,
} from "../../src/theme/ink.js";
import type { ThemeInk } from "../../src/theme/types.js";

const STYLESHEET = readFileSync(new URL("../../src/styles.css", import.meta.url), "utf8");

const varsOf = (ink: ThemeInk | undefined): Record<string, string> =>
  Object.fromEntries(inkToCssVars(ink));

describe("inkToCssVars", () => {
  it("projects every ink field onto its --th-* name", () => {
    const vars = varsOf({
      surface: { canvas: "#111111", plot: "#222222", panel: "#333333" },
      text: { primary: "#aaaaaa", secondary: "#bbbbbb", muted: "#cccccc" },
      structure: { axis: "#dddddd", grid: "#eeeeee", rule: "#ffffff" },
      series: { single: "#123456", categorical: ["#1", "#2"] },
    });

    expect(vars).toEqual({
      [THEME_CSS_VARS.canvas]: "#111111",
      [THEME_CSS_VARS.plot]: "#222222",
      [THEME_CSS_VARS.panel]: "#333333",
      [THEME_CSS_VARS.textPrimary]: "#aaaaaa",
      [THEME_CSS_VARS.textSecondary]: "#bbbbbb",
      [THEME_CSS_VARS.textMuted]: "#cccccc",
      [THEME_CSS_VARS.axis]: "#dddddd",
      [THEME_CSS_VARS.grid]: "#eeeeee",
      [THEME_CSS_VARS.rule]: "#ffffff",
      [THEME_CSS_VARS.seriesSingle]: "#123456",
      [THEME_CSS_VARS.series1]: "#1",
      [THEME_CSS_VARS.series2]: "#2",
    });
  });

  it("emits nothing for an absent ink, so useTheme clears instead of painting", () => {
    expect(inkToCssVars(undefined)).toEqual([]);
  });

  /*
   * The behaviour that makes partial customisation work: override one colour,
   * inherit the rest. An undefined field must NOT be written, because
   * `setProperty(name, undefined)` would install the string "undefined" and
   * clobber the stylesheet default it was supposed to leave alone.
   */
  it("omits undefined fields so the stylesheet default stands", () => {
    const vars = varsOf({ surface: { canvas: "#123456" } });
    expect(vars).toEqual({ [THEME_CSS_VARS.canvas]: "#123456" });
    expect(Object.keys(vars)).toHaveLength(1);
  });

  it("ignores empty strings rather than painting them", () => {
    expect(varsOf({ surface: { canvas: "" } })).toEqual({});
  });

  it("maps the categorical array onto ordered slots", () => {
    // flint treats array order as meaningful: slot 1 is the first series.
    const vars = varsOf({ series: { categorical: ["#a", "#b", "#c"] } });
    expect(vars[THEME_CSS_VARS.series1]).toBe("#a");
    expect(vars[THEME_CSS_VARS.series2]).toBe("#b");
    expect(vars[THEME_CSS_VARS.series3]).toBe("#c");
    expect(vars[THEME_CSS_VARS.series4]).toBeUndefined();
  });

  it("caps the categorical slots at the eight it reserves", () => {
    const many = Array.from({ length: 12 }, (_, index) => `#${index}`);
    const vars = varsOf({ series: { categorical: many } });
    const emitted = Object.keys(vars).filter((name) => /^--th-series-\d$/.test(name));
    expect(emitted).toHaveLength(8);
  });
});

describe("stylesheet contract", () => {
  /*
   * The failure this exists to prevent: `inkToCssVars` writes a variable, no
   * rule declares a default for it, and `useTheme`'s cleanup (which removes the
   * inline property on unmount) leaves the surface resolving an *undefined*
   * custom property -- which renders as the initial value, not the theme.
   *
   * Every name this module can emit must therefore have a default in
   * `styles.css`.
   */
  const emitted = Object.keys(
    varsOf({
      surface: { canvas: "#1", plot: "#2", panel: "#3" },
      text: { primary: "#4", secondary: "#5", muted: "#6" },
      structure: { axis: "#7", grid: "#8", rule: "#9" },
      series: {
        single: "#a",
        categorical: ["#b", "#c", "#d", "#e", "#f", "#10", "#11", "#12"],
      },
    }),
  );

  it("covers the full set of projected variables", () => {
    expect(emitted.length).toBeGreaterThanOrEqual(18);
  });

  for (const name of emitted) {
    it(`declares a default for ${name}`, () => {
      expect(STYLESHEET).toContain(`${name}:`);
    });
  }

  it("keeps the legacy aliases the unconverted rules still var()-reference", () => {
    // `.widget-card` and friends are not migrated to utilities yet and read
    // `var(--bg-card)` / `var(--text-dark)`. If these are dropped while any
    // such rule survives, the themed dashboard silently loses its surfaces.
    for (const alias of [
      "--bg-page",
      "--bg-card",
      "--bg-plot",
      "--border-card",
      "--text-dark",
      "--text-body",
      "--text-muted",
      "--primary",
      "--primary-subtle",
      "--danger",
      "--success",
    ]) {
      expect(STYLESHEET).toContain(`${alias}:`);
    }
  });

  it("keeps the accent derived from the theme's first series ink", () => {
    // `--primary` drives the shell's own buttons, so a theme that changes the
    // first categorical colour changes the accent too.
    expect(STYLESHEET).toMatch(/--primary:\s*var\(--th-series-1\)/);
  });
});

describe("ink helpers", () => {
  it('normalises hex for <input type="color">', () => {
    expect(toHexColor("#ABC")).toBe("#aabbcc");
    expect(toHexColor("aabbcc")).toBe("#aabbcc");
    expect(toHexColor("not-a-colour")).toBeNull();
  });

  it("recognises hex colours", () => {
    expect(isHexColor(" #fff ")).toBe(true);
    expect(isHexColor("rgb(0,0,0)")).toBe(false);
  });

  it("picks a readable ink for a swatch", () => {
    expect(readableInkOn("#ffffff")).toBe("#000000");
    expect(readableInkOn("#000000")).toBe("#ffffff");
  });

  it("lists categorical swatches in order, or none", () => {
    expect(categoricalSwatches({ series: { categorical: ["#1", "#2"] } })).toEqual(["#1", "#2"]);
    expect(categoricalSwatches(undefined)).toEqual([]);
    expect(categoricalSwatches({})).toEqual([]);
  });

  it("names a selection for headings", () => {
    expect(describeSelection(null)).toBe("Flint defaults");
    expect(describeSelection("power-bi")).toBe("power-bi");
    expect(describeSelection({ ink: {}, label: "Mine" })).toBe("Mine");
  });
});
