// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ChatMarkdown.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// the react-markdown root, the code block (language title, wrap and copy actions) and the link and image rules, under §53 D919: no raw HTML, http(s) links only, workspace paths as file links, images only from blob: URLs, code as token spans; no mermaid, citations, favicons, media, skills or thread links. Loams' changes are Apache-2.0.

// Studio's markdown (SD1 Task 18, inventory (b)). Model and tool output is untrusted:
// - raw HTML is text (pipeline.ts: no rehype-raw; `skipHtml={false}` turns it into text);
// - a link opens only when it is http(s), through `onOpenLink` (the platform's external
//   opener); a relative path is a file link (`onOpenFile`, the file viewer); anything
//   else is its text;
// - an image renders only from a `blob:` URL (a local attachment); a remote image is
//   its alt text, never a request;
// - code is React text spans (HighlightedTokens.tsx), never an HTML string.

import { Check, Copy, WrapText } from "lucide-react";
import {
  type ComponentProps,
  createContext,
  memo,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactMarkdown, {
  defaultUrlTransform,
  type ExtraProps as ReactMarkdownExtraProps,
} from "react-markdown";
import { HighlightedTokens } from "./HighlightedTokens";
import {
  CHAT_MARKDOWN_REHYPE_PLUGINS,
  CHAT_MARKDOWN_REMARK_PLUGINS,
  CHAT_MARKDOWN_REMARK_PLUGINS_WITH_BREAKS,
  shouldPreserveAssistantLineBreaks,
} from "./pipeline";

export interface MarkdownProps {
  text: string;
  /** The message is still streaming: a code fence may be open. */
  isStreaming?: boolean;
  /** Keep single line breaks (user text). */
  lineBreaks?: boolean;
  /** Opens an http(s) link outside the console (the platform's opener). */
  onOpenLink?: (href: string) => void;
  /** Opens a workspace file a link names, relative to the thread's checkout. */
  onOpenFile?: (path: string) => void;
  className?: string;
}

interface MarkdownHandlers {
  onOpenLink?: ((href: string) => void) | undefined;
  onOpenFile?: ((path: string) => void) | undefined;
  isStreaming: boolean;
}

const MarkdownContext = createContext<MarkdownHandlers>({ isStreaming: false });

const CODE_FENCE_LANGUAGE_REGEX = /(?:^|\s)language-([^\s]+)/;
const FENCE_TITLE_ATTR_REGEX = /(?:^|\s)(?:title|file(?:name)?)=(?:"([^"]+)"|'([^']+)'|(\S+))/i;
const SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;

/** An absolute http(s) URL, or undefined. */
export function safeHttpHref(href: string | null | undefined): string | undefined {
  if (!href) return undefined;
  try {
    const url = new URL(href.trim());
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : undefined;
  } catch {
    return undefined;
  }
}

/**
 * A workspace path a link names (`src/a.ts`, `./a.ts#L3`), or undefined. No scheme,
 * no protocol-relative `//host`, no fragment-only link, and never outside the
 * checkout (`..`); the file viewer resolves it against the thread's checkout.
 */
export function workspacePathHref(href: string | null | undefined): string | undefined {
  if (!href) return undefined;
  const raw = href.trim();
  if (raw === "" || raw.startsWith("#") || raw.startsWith("//") || SCHEME.test(raw)) {
    return undefined;
  }
  let path = raw.split(/[?#]/)[0] ?? "";
  try {
    path = decodeURIComponent(path);
  } catch {
    return undefined;
  }
  path = path.replace(/^\.\//, "");
  if (path === "" || path.startsWith("/") || path.split("/").includes("..")) return undefined;
  // oxlint-disable-next-line no-control-regex -- refusing control characters is the point
  if (/[\u0000-\u001f\u007f]/.test(path)) return undefined;
  return path;
}

function extractFenceLanguage(className: string | undefined): string {
  const match = className?.match(CODE_FENCE_LANGUAGE_REGEX);
  return match?.[1] ?? "text";
}

function extractFenceTitle(meta: string | undefined): string | null {
  if (!meta) return null;
  const match = FENCE_TITLE_ATTR_REGEX.exec(meta);
  return match?.[1] ?? match?.[2] ?? match?.[3] ?? null;
}

type HastNode = {
  type?: string;
  value?: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
};

function hastText(node: HastNode | undefined): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(hastText).join("");
}

function codeChild(node: HastNode | undefined): HastNode | undefined {
  return node?.children?.find((child) => child.type === "element" && child.tagName === "code");
}

function classNameOf(node: HastNode | undefined): string | undefined {
  const value = node?.properties?.className;
  return Array.isArray(value) ? value.join(" ") : typeof value === "string" ? value : undefined;
}

export function MarkdownCodeBlockFrame({
  language,
  fenceTitle,
  wrapped = true,
  actions,
  children,
}: {
  language: string;
  fenceTitle: string | null;
  wrapped?: boolean;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      className="my-2 overflow-hidden rounded-lg border border-rule bg-raised"
      data-language={language}
      data-wrap={wrapped ? "true" : "false"}
    >
      <div className="flex items-center justify-between gap-2 border-b border-rule-soft py-1 pr-1 pl-3 select-none">
        <span className={/* mono-ok: code */ "min-w-0 truncate font-mono text-xs text-muted"}>
          {fenceTitle ?? language}
        </span>
        {actions}
      </div>
      {children}
    </div>
  );
}

function IconButton({
  label,
  pressed,
  onClick,
  children,
}: {
  label: string;
  pressed?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onClick={onClick}
      className="inline-flex size-6 cursor-pointer items-center justify-center rounded-pill border-0 bg-transparent text-muted hover:bg-surface hover:text-ink"
    >
      {children}
    </button>
  );
}

function MarkdownCodeBlock({
  code,
  language,
  fenceTitle,
}: {
  code: string;
  language: string;
  fenceTitle: string | null;
}) {
  const [copied, setCopied] = useState(false);
  const [wrapped, setWrapped] = useState(false);
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const wrapLabel = wrapped ? "Disable line wrap" : "Wrap lines";
  const copyLabel = copied ? "Copied" : "Copy code";

  const handleCopy = useCallback(() => {
    if (typeof navigator === "undefined" || navigator.clipboard == null) return;
    void navigator.clipboard.writeText(code).then(
      () => {
        if (copiedTimerRef.current != null) clearTimeout(copiedTimerRef.current);
        setCopied(true);
        copiedTimerRef.current = setTimeout(() => {
          setCopied(false);
          copiedTimerRef.current = null;
        }, 1200);
      },
      () => {
        // The clipboard refused (no permission, no focus): nothing to undo.
      },
    );
  }, [code]);

  useEffect(
    () => () => {
      if (copiedTimerRef.current != null) {
        clearTimeout(copiedTimerRef.current);
        copiedTimerRef.current = null;
      }
    },
    [],
  );

  return (
    <MarkdownCodeBlockFrame
      language={language}
      fenceTitle={fenceTitle}
      wrapped={wrapped}
      actions={
        <span className="flex items-center gap-0.5" role="toolbar" aria-label="Code block actions">
          <IconButton label={wrapLabel} pressed={wrapped} onClick={() => setWrapped((v) => !v)}>
            <WrapText className="size-3" aria-hidden />
          </IconButton>
          <IconButton label={copyLabel} onClick={handleCopy}>
            {copied ? (
              <Check className="size-3" aria-hidden />
            ) : (
              <Copy className="size-3" aria-hidden />
            )}
          </IconButton>
        </span>
      }
    >
      <pre
        className={
          wrapped
            ? "m-0 overflow-x-auto px-3 py-2 font-mono text-xs leading-relaxed whitespace-pre-wrap" // mono-ok: code
            : "m-0 overflow-x-auto px-3 py-2 font-mono text-xs leading-relaxed whitespace-pre" // mono-ok: code
        }
      >
        <code data-code-block="">
          <HighlightedTokens code={code} language={language} />
        </code>
      </pre>
    </MarkdownCodeBlockFrame>
  );
}

function MarkdownLink({ href, children }: ComponentProps<"a"> & ReactMarkdownExtraProps) {
  const { onOpenLink, onOpenFile } = useContext(MarkdownContext);
  const external = safeHttpHref(href);
  if (external) {
    return (
      <a
        href={external}
        target="_blank"
        rel="noopener noreferrer"
        referrerPolicy="no-referrer"
        className="text-accent-fg underline underline-offset-2"
        onClick={(event) => {
          if (!onOpenLink) return;
          event.preventDefault();
          onOpenLink(external);
        }}
      >
        {children}
      </a>
    );
  }
  const path = workspacePathHref(href);
  if (path && onOpenFile) {
    return (
      <button
        type="button"
        className={
          /* mono-ok: path */ "cursor-pointer border-0 bg-transparent p-0 font-mono text-[0.95em] text-accent-fg underline underline-offset-2"
        }
        title={`Open ${path}`}
        data-file-link={path}
        onClick={() => onOpenFile(path)}
      >
        {children}
      </button>
    );
  }
  return <span data-link-refused="">{children}</span>;
}

function MarkdownImage({ src, alt }: ComponentProps<"img"> & ReactMarkdownExtraProps) {
  if (typeof src === "string" && src.startsWith("blob:")) {
    return (
      <img src={src} alt={alt ?? ""} className="my-1 max-w-[min(100%,30rem)] border border-rule" />
    );
  }
  // A remote image is never fetched (§53 D919): it would leak the reader to the host.
  return (
    <span className="text-muted italic" data-image-refused="">
      {alt ? `[image: ${alt}]` : "[image]"}
    </span>
  );
}

const CHAT_MARKDOWN_COMPONENTS = {
  a: MarkdownLink,
  img: MarkdownImage,
  pre({ node }: ComponentProps<"pre"> & ReactMarkdownExtraProps) {
    const code = codeChild(node as HastNode | undefined);
    const text = hastText(code).replace(/\n$/, "");
    const meta = code?.properties?.dataCodeMeta;
    return (
      <MarkdownCodeBlock
        code={text}
        language={extractFenceLanguage(classNameOf(code))}
        fenceTitle={extractFenceTitle(typeof meta === "string" ? meta : undefined)}
      />
    );
  },
  code({ children }: ComponentProps<"code"> & ReactMarkdownExtraProps) {
    return (
      <code className="rounded-xs bg-raised px-1 py-px font-mono text-[0.9em]">{children}</code>
    );
  },
  p: ({ children }: ComponentProps<"p">) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }: ComponentProps<"ul">) => <ul className="my-2 list-disc pl-6">{children}</ul>,
  ol: ({ children, start }: ComponentProps<"ol">) => (
    <ol className="my-2 list-decimal pl-6" start={start}>
      {children}
    </ol>
  ),
  li: ({ children }: ComponentProps<"li">) => <li className="my-0.5">{children}</li>,
  blockquote: ({ children }: ComponentProps<"blockquote">) => (
    <blockquote className="my-2 border-l-2 border-rule pl-3 text-muted">{children}</blockquote>
  ),
  h1: ({ children }: ComponentProps<"h1">) => (
    <h1 className="mt-4 mb-2 text-lg font-semibold">{children}</h1>
  ),
  h2: ({ children }: ComponentProps<"h2">) => (
    <h2 className="mt-4 mb-2 text-base font-semibold">{children}</h2>
  ),
  h3: ({ children }: ComponentProps<"h3">) => (
    <h3 className="mt-3 mb-1 text-sm font-semibold">{children}</h3>
  ),
  h4: ({ children }: ComponentProps<"h4">) => (
    <h4 className="mt-3 mb-1 text-sm font-semibold">{children}</h4>
  ),
  h5: ({ children }: ComponentProps<"h5">) => (
    <h5 className="mt-3 mb-1 text-sm font-semibold">{children}</h5>
  ),
  h6: ({ children }: ComponentProps<"h6">) => (
    <h6 className="mt-3 mb-1 text-sm font-semibold">{children}</h6>
  ),
  hr: () => <hr className="my-3 border-0 border-t border-rule" />,
  table: ({ children }: ComponentProps<"table">) => (
    <div className="my-2 overflow-x-auto">
      <table className="border-collapse text-xs">{children}</table>
    </div>
  ),
  th: ({ children }: ComponentProps<"th">) => (
    <th className="border border-rule bg-raised px-2 py-1 text-left font-semibold">{children}</th>
  ),
  td: ({ children }: ComponentProps<"td">) => (
    <td className="border border-rule px-2 py-1">{children}</td>
  ),
};

/**
 * react-markdown's URL filter, plus `blob:` for an image's `src` (a local attachment).
 * Everything else still goes through the default filter and then the components.
 */
export function markdownUrlTransform(url: string, key: string): string {
  if (key === "src" && url.startsWith("blob:")) return url;
  return defaultUrlTransform(url);
}

function ChatMarkdown({
  text,
  className,
  lineBreaks = false,
  isStreaming = false,
  onOpenLink,
  onOpenFile,
}: MarkdownProps) {
  const handlers = useMemo(
    () => ({ onOpenLink, onOpenFile, isStreaming }),
    [onOpenLink, onOpenFile, isStreaming],
  );
  const remarkPlugins =
    lineBreaks || shouldPreserveAssistantLineBreaks(text)
      ? CHAT_MARKDOWN_REMARK_PLUGINS_WITH_BREAKS
      : CHAT_MARKDOWN_REMARK_PLUGINS;

  // react-markdown turns HTML it does not parse into text when skipHtml is false;
  // without rehype-raw nothing is parsed, so all HTML in the source shows as text.
  return (
    <div
      className={
        className
          ? `chat-markdown min-w-0 text-sm leading-relaxed text-ink [overflow-wrap:anywhere] ${className}`
          : "chat-markdown min-w-0 text-sm leading-relaxed text-ink [overflow-wrap:anywhere]"
      }
      data-streaming={isStreaming ? "" : undefined}
    >
      <MarkdownContext value={handlers}>
        <ReactMarkdown
          remarkPlugins={remarkPlugins}
          rehypePlugins={CHAT_MARKDOWN_REHYPE_PLUGINS}
          skipHtml={false}
          components={CHAT_MARKDOWN_COMPONENTS}
          urlTransform={markdownUrlTransform}
        >
          {text}
        </ReactMarkdown>
      </MarkdownContext>
    </div>
  );
}

export const Markdown = memo(ChatMarkdown);
