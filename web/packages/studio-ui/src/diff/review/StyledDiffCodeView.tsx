// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/diffs/StyledDiffCodeView.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// one file per viewer, driving @pierre/diffs' FileDiff class from an effect on the shadow-root path in place of the React CodeView wrapper (T0-4, T21-2), with no worker pool or virtualizer, the token theme of diffTheme.ts, and annotations portalled into their slots; the selection, separator and motion rules kept, the header rules dropped with Pierre's header. Loams' changes are Apache-2.0.

// One file of a diff, highlighted (SD1 Task 20; design §53 D919, ruling T0-4).
// @pierre/diffs tokenises the patch with Shiki and writes escaped text nodes into an open
// shadow root: a patch's text is never parsed as HTML. Studio uses only this path: no
// `prerenderedHTML`, no SSR, no marker `{html}` messages, no `allowDangerousHtml`.
//
// The FileDiff class is driven from an effect rather than through `@pierre/diffs/react`, as
// Task 21's viewer is (T21-2): each mount makes its own `<diffs-container>`, so React's
// development remount never finds a previous instance's empty tree. The container is
// "managed": the class leaves a named <slot> for each annotation row, and React portals the
// comment UI into the container's light DOM with that slot name, as the wrapper would.

import {
  type DiffLineAnnotation,
  type FileDiffContentsLoader,
  type FileDiffMetadata,
  type FileDiffOptions,
  getLineAnnotationName,
  FileDiff as PierreFileDiff,
  type SelectedLineRange,
} from "@pierre/diffs";
import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { ensureCodeTheme } from "../../files/codeTheme";
import { diffSurfaceCss } from "../diffTheme";

/** The app's diff stylesheet inside each file's shadow root: our static rules only. */
export const DIFF_VIEW_UNSAFE_CSS = `
:is(
  [data-line],
  [data-line-annotation],
  [data-no-newline]
)[data-selected-line] {
  --diffs-line-bg: color-mix(in srgb, var(--code-background) 80%, var(--op-accent-soft)) !important;
}

:is([data-gutter-buffer], [data-column-number])[data-selected-line] {
  --diffs-line-bg: var(--op-accent-soft) !important;
}

/* Hunk separators are the way to the unchanged lines between hunks: quiet controls one step
   off the code surface; hover brings them forward. */
:is([data-separator="line-info"], [data-separator="line-info-basic"]) {
  height: 24px !important;
  margin-block: 0 !important;
  --diffs-bg-separator: color-mix(in srgb, var(--code-background) 97%, var(--code-foreground));
}

:is([data-separator="line-info"], [data-separator="line-info-basic"]) [data-separator-content] {
  color: var(--op-faint) !important;
  font-family: var(--loams-font-sans) !important;
  font-size: 11px !important;
  text-decoration: none !important;
}

:is([data-separator="line-info"], [data-separator="line-info-basic"]) [data-expand-button] {
  color: var(--op-faint) !important;
}

:is([data-separator="line-info"], [data-separator="line-info-basic"])[data-expand-index]
  :is([data-separator-content], [data-expand-button]):hover {
  color: var(--code-foreground) !important;
  background-color: var(--op-raised) !important;
}

[data-diff],
[data-file] {
  transition: opacity 200ms ease-out;
}

@starting-style {
  [data-diff],
  [data-file] {
    opacity: 0;
  }
}

@media (prefers-reduced-motion: reduce) {
  [data-diff],
  [data-file] {
    transition: none;
  }
}
`;

export interface StyledDiffCodeViewOptions {
  readonly layout: "unified" | "split";
  readonly wrap: boolean;
  readonly wordDiff: boolean;
  readonly scheme: "light" | "dark";
  /** Shows the gutter's "+" and allows dragging a range of lines. */
  readonly commentable: boolean;
  readonly loadDiffFiles?: FileDiffContentsLoader;
}

export function StyledDiffCodeView<LAnnotation>({
  fileDiff,
  annotations,
  renderAnnotation,
  options,
  selectedLines,
  onSelectLines,
  className,
}: {
  readonly fileDiff: FileDiffMetadata;
  readonly annotations: readonly DiffLineAnnotation<LAnnotation>[];
  readonly renderAnnotation: (annotation: DiffLineAnnotation<LAnnotation>) => ReactNode;
  readonly options: StyledDiffCodeViewOptions;
  /** The range the reader is commenting on, kept highlighted. */
  readonly selectedLines: SelectedLineRange | null;
  /** The gutter's "+" or a dragged range asks for a comment on these lines. */
  readonly onSelectLines: (range: SelectedLineRange) => void;
  readonly className?: string;
}) {
  const wrapper = useRef<HTMLDivElement>(null);
  const instance = useRef<PierreFileDiff<LAnnotation> | null>(null);
  const [host, setHost] = useState<HTMLElement | null>(null);
  const onSelect = useRef(onSelectLines);
  useEffect(() => {
    onSelect.current = onSelectLines;
  }, [onSelectLines]);

  const { layout, wrap, wordDiff, scheme, commentable, loadDiffFiles } = options;
  const pierreOptions = useMemo<FileDiffOptions<LAnnotation, undefined>>(
    () => ({
      theme: ensureCodeTheme(),
      themeType: scheme,
      // The WASM engine: Shiki's JS regex engine can backtrack without end on hostile input.
      preferredHighlighter: "shiki-wasm",
      diffStyle: layout,
      overflow: wrap ? "wrap" : "scroll",
      lineDiffType: wordDiff ? "word-alt" : "none",
      disableFileHeader: true,
      hunkSeparators: "line-info",
      unsafeCSS: `${diffSurfaceCss(scheme)}\n${DIFF_VIEW_UNSAFE_CSS}`,
      enableGutterUtility: commentable,
      enableLineSelection: commentable,
      controlledSelection: true,
      onGutterUtilityClick: (range) => onSelect.current(range),
      onLineSelectionEnd: (range) => {
        if (range && range.start !== range.end) onSelect.current(range);
      },
      ...(loadDiffFiles ? { loadDiffFiles } : {}),
    }),
    [commentable, layout, loadDiffFiles, scheme, wordDiff, wrap],
  );

  // Each mount owns its container and instance.
  // oxlint-disable-next-line react/exhaustive-deps -- created once; options follow below.
  useLayoutEffect(() => {
    const parent = wrapper.current;
    if (!parent) return;
    const container = document.createElement("diffs-container");
    parent.append(container);
    const created = new PierreFileDiff<LAnnotation>(pierreOptions, undefined, true);
    instance.current = created;
    setHost(container);
    return () => {
      created.cleanUp();
      container.remove();
      if (instance.current === created) instance.current = null;
    };
  }, []);

  const rendered = useRef<FileDiffOptions<LAnnotation, undefined> | null>(null);
  useLayoutEffect(() => {
    const current = instance.current;
    if (!current || !host) return;
    const optionsChanged = rendered.current !== pierreOptions;
    rendered.current = pierreOptions;
    current.setOptions(pierreOptions);
    current.render({
      fileDiff,
      fileContainer: host,
      lineAnnotations: [...annotations],
      forceRender: optionsChanged,
    });
  }, [annotations, fileDiff, host, pierreOptions]);

  useLayoutEffect(() => {
    instance.current?.setSelectedLines(selectedLines, { notify: false });
  }, [selectedLines]);

  return (
    <div ref={wrapper} className={className} data-diff-file-view>
      {host
        ? createPortal(
            annotations.map((annotation) => (
              <div
                key={`${annotation.side}:${annotation.lineNumber}`}
                slot={getLineAnnotationName(annotation)}
              >
                {renderAnnotation(annotation)}
              </div>
            )),
            host,
          )
        : null}
    </div>
  );
}
