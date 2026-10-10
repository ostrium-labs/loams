import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  type Loaded,
  NamespacePicker,
  PageHead,
  recentNamespaces,
  rememberNamespace,
  useLoad,
} from '../src/index.js';

afterEach(() => {
  cleanup();
  localStorage.clear();
});

function Probe({
  load,
  dep,
  keepOnReload,
  errorText,
  onReload,
}: {
  load: () => Promise<string>;
  dep: string;
  keepOnReload?: boolean;
  errorText?: (e: unknown) => string;
  onReload: (r: () => void) => void;
}) {
  const [v, reload] = useLoad(load, [dep], { keepOnReload, errorText });
  onReload(reload);
  const s = v as Loaded<string>;
  return <p>{s.state === 'ready' ? s.data : s.state === 'error' ? `E:${s.message}` : 'loading'}</p>;
}

describe('useLoad', () => {
  it('loads_and_reports_errors_through_errorText', async () => {
    let reload = () => {};
    const { rerender } = render(
      <Probe load={async () => 'one'} dep="a" onReload={(r) => (reload = r)} />,
    );
    expect(await screen.findByText('one')).toBeTruthy();
    rerender(
      <Probe
        load={async () => {
          throw new Error('boom');
        }}
        dep="b"
        errorText={(e) => `wrapped ${(e as Error).message}`}
        onReload={(r) => (reload = r)}
      />,
    );
    expect(await screen.findByText('E:wrapped boom')).toBeTruthy();
    void reload;
  });

  it('keep_on_reload_keeps_the_data_in_view', async () => {
    let reload = () => {};
    let n = 0;
    let release: () => void = () => {};
    const load = () =>
      n++ === 0
        ? Promise.resolve('first')
        : new Promise<string>((r) => {
            release = () => r('second');
          });
    render(<Probe load={load} dep="a" keepOnReload onReload={(r) => (reload = r)} />);
    expect(await screen.findByText('first')).toBeTruthy();
    act(() => reload());
    expect(screen.getByText('first')).toBeTruthy();
    await act(async () => release());
    expect(await screen.findByText('second')).toBeTruthy();
  });
});

describe('PageHead', () => {
  it('renders_crumbs_title_subtitle_actions', () => {
    render(
      <PageHead
        title="Streams"
        subtitle="sub"
        crumbs={<a href="#/x">Back</a>}
        actions={<button type="button">Act</button>}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Streams' })).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Back' })).toBeTruthy();
    expect(screen.getByText('sub')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Act' })).toBeTruthy();
  });
});

describe('NamespacePicker', () => {
  it('opens_the_typed_namespace_and_lists_recent_ones', () => {
    rememberNamespace('k', 'alpha');
    rememberNamespace('k', 'beta');
    expect(recentNamespaces('k')).toEqual(['beta', 'alpha']);
    const opened: string[] = [];
    const { container } = render(
      <NamespacePicker ns="default" storageKey="k" idPrefix="t" onOpen={(n) => opened.push(n)} />,
    );
    const input = screen.getByLabelText('Namespace');
    fireEvent.change(input, { target: { value: '  gamma ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Open' }));
    expect(opened).toEqual(['gamma']);
    const opts = [...container.querySelectorAll('datalist option')].map((o) =>
      o.getAttribute('value'),
    );
    expect(opts).toEqual(['default', 'beta', 'alpha']);
  });
});
