/**
 * `cn` -- the app's class-name helper.
 *
 * Tested from `dashboard-ui` rather than from core because the thing worth
 * pinning is the contract *dashboard-ui's* copied components rely on: the
 * Tailwind merge is extended with the theme's extra font sizes, so
 * `text-2xs` survives alongside a colour utility instead of being treated as a
 * colour and dropped.
 */
import { describe, expect, it } from "vite-plus/test";
import { cn } from "@loams-plugins/core/ui";

describe("cn", () => {
  it("joins class names", () => {
    expect(cn("a", "b")).toBe("a b");
  });

  it("drops falsy values", () => {
    expect(cn("a", false, undefined, null, "", "b")).toBe("a b");
  });

  it("flattens arrays and objects, clsx-style", () => {
    expect(cn(["a", "b"], { c: true, d: false })).toBe("a b c");
  });

  it("lets the last conflicting Tailwind utility win", () => {
    expect(cn("px-2", "px-4")).toBe("px-4");
    expect(cn("text-sm", "text-lg")).toBe("text-lg");
  });

  /*
   * The load-bearing case. Without the `extendTailwindMerge` registration in
   * `cn.ts`, `text-2xs` is unreadable as a font size, so tailwind-merge treats
   * the *later* `text-muted-foreground` as the conflicting colour and discards
   * `text-2xs` -- which would silently change the type size on every one of the
   * ported t3code components.
   */
  it("keeps the theme's sub-xs font sizes alongside a colour", () => {
    expect(cn("text-2xs", "text-muted-foreground")).toBe("text-2xs text-muted-foreground");
    expect(cn("text-3xs text-ink-body", "text-4xs")).toBe("text-ink-body text-4xs");
  });

  it("still resolves a real conflict between two of those sizes", () => {
    expect(cn("text-2xs", "text-3xs")).toBe("text-3xs");
  });

  it("resolves font-size against weight, not against each other", () => {
    expect(cn("text-2xs", "font-semibold")).toBe("text-2xs font-semibold");
  });

  it("does not treat a colour as overriding an arbitrary property", () => {
    expect(cn("bg-primary-subtle", "text-ink")).toBe("bg-primary-subtle text-ink");
  });
});
