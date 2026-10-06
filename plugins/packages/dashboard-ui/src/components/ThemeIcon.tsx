import React from "react";
import { cn } from "@loams-plugins/core/ui";

/** Opaque to CSS by design (see below): a fixed-size swatch, not a themed mark. */
const ICON = "block max-h-full max-w-full";

const FALLBACK = `${ICON} rounded-[2px] border border-dashed border-ink-subtle`;

interface ThemeIconProps {
  /** A complete SVG document from the theme service. */
  svg: string;
  /** Used for the alt text and the accessible name of the owning control. */
  label: string;
  size?: number;
}

/**
 * Renders a theme preset's icon.
 *
 * Approach: data-URI `<img>`. These `icon` strings are complete SVG documents
 * from the server, and `dangerouslySetInnerHTML` would inject them into the
 * live DOM of the host page — that runs their `<script>`, their event handler
 * attributes and any `<foreignObject>` markup with the page's full privileges.
 * A `data:image/svg+xml` URI in an `<img>` is rendered as an image instead:
 * script in an `<img>`-loaded SVG does not execute, external references are not
 * fetched, and the document cannot reach back into this page. It also keeps the
 * content out of the accessibility tree as markup while `alt` carries the
 * meaning.
 *
 * The trade-off is that the image is opaque to CSS and to theming, which is
 * acceptable for a fixed 16px swatch. Anything that needs to react to the
 * dashboard theme draws its own SVG instead.
 *
 * If a preset arrives with an empty or unparseable icon, the fallback keeps the
 * grid legible rather than showing a broken-image glyph.
 */
export const ThemeIcon: React.FC<ThemeIconProps> = ({ svg, label, size = 20 }) => {
  if (!svg || !svg.trim()) {
    return (
      <span
        className={cn(FALLBACK, "inline-block")}
        style={{ width: size, height: size }}
        aria-hidden="true"
      />
    );
  }

  return (
    <img
      className={ICON}
      src={`data:image/svg+xml;utf8,${encodeURIComponent(svg)}`}
      alt={label}
      width={size}
      height={size}
      loading="lazy"
    />
  );
};

/**
 * A local stand-in for flint's `DEFAULT_THEME_ICON`, which the theme service
 * does not return on `GET /api/themes` (that endpoint lists the ten houses, and
 * "no house" is not one of them). Without it the "Flint defaults" choice would
 * be an empty slot, which is exactly what flint exports the constant to avoid.
 */
export const DefaultThemeIcon: React.FC<{ size?: number }> = ({ size = 20 }) => (
  <svg
    className={cn(ICON, "inline-block")}
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.4"
    strokeLinecap="round"
    role="img"
    aria-label="Flint defaults"
  >
    <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" />
    <line x1="1.5" y="10.5" x2="14.5" y2="10.5" />
    <path d="M4 8.5h3" />
  </svg>
);
