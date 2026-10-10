// Safe Markdown for model output (D675, controller ruling).
//
// - No raw HTML: `<...>` in the text is shown as text, never parsed.
// - No remote images: an image renders as its alt text only (images are an
//   exfiltration channel under prompt injection).
// - Links are http(s) only; the panel opens them through desktop.shell.openExternal.
// - The result still goes through DOMPurify with an allow-list, as a second fence.
//
// Class names below are full literal strings so Tailwind generates them.

import createDOMPurify from 'dompurify';
import { Marked, type Tokens } from 'marked';

const ESCAPES: Record<string, string> = {
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
  "'": '&#39;',
};
export const escapeHtml = (s: string): string => s.replace(/[&<>"']/g, (c) => ESCAPES[c] ?? c);

/** `href` when it is an absolute http or https URL, else undefined. */
export function safeHref(href: string | null | undefined): string | undefined {
  if (!href) return undefined;
  try {
    const u = new URL(href.trim());
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.href : undefined;
  } catch {
    return undefined;
  }
}

// A private instance: its hooks and config never touch other plugins' DOMPurify.
const DOMPurify = createDOMPurify(window);

const marked = new Marked({
  async: false,
  gfm: true,
  breaks: false,
  renderer: {
    html: ({ text }: Tokens.HTML | Tokens.Tag) => escapeHtml(text),
    image: ({ text }: Tokens.Image) => (text ? escapeHtml(text) : ''),
    link({ href, title, tokens }: Tokens.Link) {
      const inner = this.parser.parseInline(tokens);
      const safe = safeHref(href);
      if (!safe) return inner;
      const t = title ? ` title="${escapeHtml(title)}"` : '';
      return `<a href="${escapeHtml(safe)}"${t} class="text-accent underline">${inner}</a>`;
    },
    code({ text, lang }: Tokens.Code) {
      const label = (lang ?? '').trim().split(/\s+/)[0] ?? '';
      return (
        '<div class="lc-code my-2 border border-rule bg-raised">' +
        '<div class="flex items-center justify-between px-2 py-1 border-b border-rule-soft text-xs text-muted">' +
        `<span class="font-mono">${escapeHtml(label)}</span>` +
        '<button type="button" data-copy="" class="text-xs text-ink bg-surface border border-rule px-2 py-0.5 cursor-pointer">Copy</button>' +
        '</div>' +
        `<pre class="m-0 p-2 overflow-x-auto text-xs font-mono"><code>${escapeHtml(text)}</code></pre>` +
        '</div>'
      );
    },
    codespan: ({ text }: Tokens.Codespan) =>
      `<code class="font-mono text-xs bg-raised px-1">${escapeHtml(text)}</code>`,
  },
});

const ALLOWED_TAGS = [
  'p',
  'br',
  'hr',
  'strong',
  'em',
  'del',
  'code',
  'pre',
  'a',
  'ul',
  'ol',
  'li',
  'blockquote',
  'h1',
  'h2',
  'h3',
  'h4',
  'h5',
  'h6',
  'table',
  'thead',
  'tbody',
  'tr',
  'th',
  'td',
  'div',
  'span',
  'button',
  'input',
];

let hooked = false;
function ensureHooks(): void {
  if (hooked) return;
  hooked = true;
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      node.setAttribute('rel', 'noopener noreferrer');
      node.removeAttribute('target');
    }
    // Task-list checkboxes from GFM: shown, never interactive.
    if (node.tagName === 'INPUT') {
      node.setAttribute('disabled', '');
      node.setAttribute('type', 'checkbox');
    }
  });
}

/** Model Markdown as sanitised HTML, safe for `dangerouslySetInnerHTML`. */
export function renderMarkdown(text: string): string {
  ensureHooks();
  const html = (marked.parse(text) as string)
    .replace(/<table>/g, '<table class="border-collapse text-xs my-2">')
    .replace(/<(th|td)(?=[ >])/g, '<$1 class="border border-rule px-2 py-1 text-left"');
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR: ['href', 'title', 'class', 'type', 'data-copy', 'checked', 'disabled', 'align'],
    ALLOW_DATA_ATTR: false,
    ALLOWED_URI_REGEXP: /^https?:/i,
    FORBID_TAGS: ['img', 'style', 'script', 'iframe', 'form', 'svg', 'math'],
    FORBID_ATTR: ['style', 'src', 'srcset', 'onerror', 'onload'],
  });
}
