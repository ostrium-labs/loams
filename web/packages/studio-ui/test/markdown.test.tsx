// Studio's markdown renders untrusted text as text (SD1 Task 18; §53 D919; Review Focus 4).
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';

// The highlighter: tokens whose text is markup, to prove tokens are text nodes too.
vi.mock('../src/markdown/highlight', () => ({
  highlightToTokens: vi.fn(async (code: string) => [
    [{ content: code, offset: 0, color: 'var(--loams-code-token-keyword, var(--op-accent))' }],
  ]),
}));

import {
  createMarkdownFileRenderer,
  Markdown,
  resolveMarkdownFileLink,
  safeHttpHref,
  workspacePathHref,
} from '../src/markdown';

afterEach(cleanup);

describe('markdown_no_raw_html', () => {
  it('shows raw HTML as text and creates none of its elements', () => {
    const text = [
      '<script>alert(1)</script>',
      '',
      '<img src="x" onerror="alert(2)">',
      '',
      'Inline <b onclick="alert(3)">bold</b> and <iframe src="https://evil.example"></iframe>',
      '',
      '<div style="position:fixed">overlay</div>',
    ].join('\n');
    const { container } = render(<Markdown text={text} />);
    for (const tag of ['script', 'img', 'b', 'iframe', 'div[style]', '[onerror]', '[onclick]']) {
      expect(container.querySelector(`.chat-markdown ${tag}`), tag).toBeNull();
    }
    expect(container.textContent).toContain('<script>alert(1)</script>');
    expect(container.textContent).toContain('<b onclick="alert(3)">');
  });
});

describe('markdown_no_remote_images', () => {
  it('renders a remote image as its alt text and never requests it', () => {
    const { container } = render(
      <Markdown text={'![cat](https://evil.example/cat.png) ![](http://evil.example/x.gif)'} />,
    );
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('[image: cat]');
    expect(container.querySelectorAll('[data-image-refused]')).toHaveLength(2);
  });

  it('renders a local blob: image', () => {
    const { container } = render(<Markdown text={'![shot](blob:http://localhost/1234)'} />);
    const img = container.querySelector('img');
    expect(img?.getAttribute('src')).toBe('blob:http://localhost/1234');
    expect(img?.getAttribute('alt')).toBe('shot');
  });

  it('refuses data: and file: images', () => {
    const { container } = render(
      <Markdown text={'![a](data:image/png;base64,AAAA) ![b](file:///etc/passwd)'} />,
    );
    expect(container.querySelector('img')).toBeNull();
  });
});

describe('markdown_links_http_only', () => {
  it('links http(s) only, through the external opener', () => {
    const onOpenLink = vi.fn();
    const { container, getByText } = render(
      <Markdown
        text={[
          '[site](https://example.com/a?b=1)',
          '[plain](http://example.com)',
          '[js](javascript:alert(1))',
          '[file](file:///etc/passwd)',
          '[mail](mailto:a@example.com)',
          '[data](data:text/html,<b>x</b>)',
          '[vb](vbscript:msgbox)',
        ].join(' ')}
        onOpenLink={onOpenLink}
      />,
    );
    const anchors = [...container.querySelectorAll('a')];
    expect(anchors.map((a) => a.getAttribute('href'))).toEqual([
      'https://example.com/a?b=1',
      'http://example.com/',
    ]);
    for (const a of anchors) {
      expect(a.getAttribute('rel')).toBe('noopener noreferrer');
      expect(a.getAttribute('target')).toBe('_blank');
    }
    fireEvent.click(getByText('site'));
    expect(onOpenLink).toHaveBeenCalledWith('https://example.com/a?b=1');
    for (const label of ['js', 'file', 'mail', 'data', 'vb']) {
      expect(getByText(label).closest('a'), label).toBeNull();
    }
  });

  it('opens workspace paths in the file viewer, never outside the checkout', () => {
    const onOpenFile = vi.fn();
    const { getByText } = render(
      <Markdown
        text={'[main](src/main.rs#L3) [up](../secret) [abs](/etc/passwd) [frag](#top)'}
        onOpenFile={onOpenFile}
      />,
    );
    fireEvent.click(getByText('main'));
    expect(onOpenFile).toHaveBeenCalledWith('src/main.rs');
    for (const label of ['up', 'abs', 'frag']) {
      expect(getByText(label).closest('button'), label).toBeNull();
    }
  });

  it('classifies hrefs', () => {
    expect(safeHttpHref('https://a.example')).toBe('https://a.example/');
    expect(safeHttpHref('javascript:alert(1)')).toBeUndefined();
    expect(safeHttpHref('//evil.example')).toBeUndefined();
    expect(workspacePathHref('./a/b.ts')).toBe('a/b.ts');
    expect(workspacePathHref('a/%2e%2e/b')).toBeUndefined();
    expect(workspacePathHref('//evil.example/x')).toBeUndefined();
    expect(workspacePathHref('C:/x')).toBeUndefined();
  });
});

describe('code_block_renders_text_not_html', () => {
  it('renders fenced code, highlighted, as text nodes', async () => {
    const code = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
    const { container } = render(<Markdown text={`\`\`\`html\n${code}\n\`\`\``} />);
    const block = container.querySelector('[data-code-block]');
    expect(block?.textContent).toBe(code);
    await waitFor(() => expect(block?.querySelector('[data-line] span')).not.toBeNull());
    expect(block?.textContent).toBe(code);
    expect(container.querySelector('.chat-markdown img, .chat-markdown script')).toBeNull();
    // The language title and the copy action.
    expect(container.querySelector('[data-language="html"]')).not.toBeNull();
    expect(container.querySelector('button[aria-label="Copy code"]')).not.toBeNull();
  });

  it('renders inline code as text', () => {
    const { container } = render(<Markdown text={'Run `<b>rm -rf</b>` now'} />);
    expect(container.querySelector('code')?.textContent).toBe('<b>rm -rf</b>');
    expect(container.querySelector('.chat-markdown b')).toBeNull();
  });

  it('copies the code', async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const { getByLabelText } = render(<Markdown text={'```sh\nls -la\n```'} />);
    fireEvent.click(getByLabelText('Copy code'));
    expect(writeText).toHaveBeenCalledWith('ls -la');
    await waitFor(() => expect(getByLabelText('Copied')).toBeTruthy());
  });
});

describe('markdown files in the file viewer (T21-3)', () => {
  it('renders a document sanitised, with links relative to the file', () => {
    const onOpenFile = vi.fn();
    const renderFile = createMarkdownFileRenderer({ onOpenFile });
    const { container, getByText } = render(
      <div>
        {renderFile(
          '# Guide\n\n<script>x</script>\n\n[api](api.md) [up](../x.md)',
          'docs/guide.md',
        )}
      </div>,
    );
    expect(container.querySelector('h1')?.textContent).toBe('Guide');
    expect(container.querySelector('script')).toBeNull();
    fireEvent.click(getByText('api'));
    expect(onOpenFile).toHaveBeenCalledWith('docs/api.md');
    expect(getByText('up').closest('button')).toBeNull();
    expect(resolveMarkdownFileLink('README.md', 'a/b.md')).toBe('a/b.md');
  });
});
