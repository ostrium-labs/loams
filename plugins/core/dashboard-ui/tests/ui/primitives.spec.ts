/**
 * The ported t3code primitives.
 *
 * These files are copied verbatim from t3code 0.0.45 (MIT, T3 Tools Inc 2026)
 * with two changes: `cn` comes from this repo's own `../cn.ts` instead of
 * t3code's `~/lib/utils` (which pulled in `@t3tools/contracts` and `effect`),
 * and they live in `core/host/src/ui/primitives` rather than here, because
 * the plugin console in `@loams-core/host` renders one of them and the dependency only
 * points one way. `@loams-core/host/ui` re-exports all nine.
 *
 * What is worth testing here is the thing a copy can silently lose: the token
 * names in their class strings (`--control-radius`, `--ring`, `--background`, and
 * the `text-2xs`..`text-5xs` sizes registered only through `cn`). If a token
 * stops being declared in `styles.css` the component renders unstyled and
 * nothing throws -- so these tests assert the tokens are actually reachable
 * from the stylesheet.
 */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import * as coreUi from "../../../host/src/ui/index.js";

const STYLESHEET = readFileSync(new URL("../../src/styles.css", import.meta.url), "utf8");

/**
 * Tailwind only emits a utility if it finds a complete class name in the
 * source. A class built by interpolation (`text-[${colour}]`) therefore compiles
 * to nothing, which is a silent failure -- so the copied components' token
 * classes have to be present as literals.
 */
describe("ported primitive tokens", () => {
  /*
   * Named `--color-*`, which is the namespace Tailwind v4 turns into colour
   * utilities: `--color-ring` is what makes `ring-ring` resolve. The copied
   * components ask for those utility names (`text-muted-foreground`,
   * `border-border`, `bg-background`), so this is the list that has to exist --
   * a bare `--ring` would generate nothing.
   */
  const tokens = [
    "--color-background",
    "--color-foreground",
    "--color-muted",
    "--color-muted-foreground",
    "--color-accent",
    "--color-accent-foreground",
    "--color-card",
    "--color-card-foreground",
    "--color-popover",
    "--color-secondary",
    "--color-destructive",
    "--color-input",
    "--color-border",
    "--color-ring",
    "--color-primary",
    // Read directly by Button/Toggle rather than through a utility.
    "--control-radius",
    // The sub-xs type scale, registered with tailwind-merge via `cn`.
    "--text-2xs",
    "--text-3xs",
    "--text-4xs",
    "--text-5xs",
  ];

  for (const token of tokens) {
    it(`declares ${token}`, () => {
      expect(STYLESHEET).toContain(`${token}:`);
    });
  }

  it("uses the classes that depend on those tokens", () => {
    /*
     * A guard on the guard: if the copied components were dropped, or their
     * class strings refactored away, the token declarations alone would still
     * pass. These are the token-backed utilities the primitives actually
     * reference, so their presence proves the pair is still wired together.
     */
    const sources = ["button", "switch", "badge", "empty", "alert", "skeleton"]
      .map((name) =>
        readFileSync(
          new URL(`../../../host/src/ui/primitives/${name}.tsx`, import.meta.url),
          "utf8",
        ),
      )
      .join("\n");

    for (const utility of [
      "var(--control-radius)",
      "ring-ring",
      "ring-offset-background",
      "bg-background",
      "text-muted-foreground",
      "border-border",
    ]) {
      expect(sources).toContain(utility);
    }
  });

  it("imports Tailwind, so preflight and the utilities exist", () => {
    expect(STYLESHEET).toContain('@import "tailwindcss"');
  });

  it("declares its theme inline, so runtime ink changes still reach utilities", () => {
    // A plain `@theme` would bake values into the generated utilities and the
    // `--th-*` writes `useTheme` makes on <html> at runtime would stop working.
    expect(STYLESHEET).toContain("@theme inline");
  });

  it("keeps the chart-specs ink layer as the source of the theme", () => {
    expect(STYLESHEET).toContain("--th-canvas");
    expect(STYLESHEET).toContain("--th-text-primary");
  });
});

describe("MIT attribution", () => {
  const files = [
    "alert",
    "badge",
    "button",
    "empty",
    "kbd",
    "label",
    "separator",
    "skeleton",
    "switch",
  ];

  for (const name of files) {
    it(`retains the licence header in ${name}.tsx`, () => {
      const source = readFileSync(
        new URL(`../../../host/src/ui/primitives/${name}.tsx`, import.meta.url),
        "utf8",
      );
      expect(source).toContain("T3 Tools Inc");
      expect(source).toContain("Permission is hereby granted, free of charge");
      expect(source).toContain("WITHOUT WARRANTY OF ANY KIND");
    });
  }

  it("does not import t3code's contracts/effect-coupled utils", () => {
    for (const name of files) {
      const source = readFileSync(
        new URL(`../../../host/src/ui/primitives/${name}.tsx`, import.meta.url),
        "utf8",
      );
      expect(source).not.toContain("@t3tools/contracts");
      expect(source).not.toContain('from "effect');
      // Matched as an import specifier, not as prose: each file's header
      // deliberately *mentions* the `~/lib/utils` import it replaced.
      expect(source).not.toContain('from "~/lib/utils"');
      expect(source).not.toContain("from '~/lib/utils'");
    }
  });

  it("skips the primitives that are coupled to react-router/effect", () => {
    // Deliberately not copied: toast, sidebar, and the @pierre/diffs viewer
    // (whose licence was never verified).
    for (const name of ["toast", "sidebar", "diff"]) {
      const exists = (() => {
        try {
          readFileSync(new URL(`../../src/components/ui/${name}.tsx`, import.meta.url));
          return true;
        } catch {
          return false;
        }
      })();
      expect(exists).toBe(false);
    }
  });
});

describe("the primitives' home", () => {
  /*
   * They were moved into `@loams-core/host` because the plugin console renders `Switch`
   * and `@loams-core/host` sits below the app in the dependency graph. Asserting the
   * re-export surface keeps the move from quietly orphaning one: a primitive
   * that stops being exported is only visible as a build error somewhere else,
   * and only for whoever next reaches for it.
   */
  const exported = {
    alert: ["Alert", "AlertAction", "AlertDescription", "AlertTitle"],
    badge: ["Badge"],
    button: ["Button", "InlineButton"],
    empty: ["Empty", "EmptyContent", "EmptyDescription", "EmptyHeader", "EmptyMedia", "EmptyTitle"],
    kbd: ["Kbd", "KbdGroup"],
    label: ["Label"],
    separator: ["Separator"],
    skeleton: ["Skeleton"],
    switch: ["Switch"],
  } as const;

  for (const [name, symbols] of Object.entries(exported)) {
    for (const symbol of symbols) {
      it(`@loams-core/host/ui re-exports ${symbol} (from ${name}.tsx)`, () => {
        expect(typeof (coreUi as Record<string, unknown>)[symbol]).not.toBe("undefined");
      });
    }
  }

  it("has no second copy of the primitives left in the app", () => {
    // Two copies would drift, and the app's would be the one nothing imports.
    for (const name of Object.keys(exported)) {
      const exists = (() => {
        try {
          readFileSync(new URL(`../../src/components/ui/${name}.tsx`, import.meta.url));
          return true;
        } catch {
          return false;
        }
      })();
      expect(exists).toBe(false);
    }
  });
});
