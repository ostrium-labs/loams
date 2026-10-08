import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { GraphPage } from '../src/graph-page.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const desktop = () => {
  const opened: string[] = [];
  return {
    opened,
    api: {
      shell: {
        openExternal: async (u: string) => {
          opened.push(u);
          return { ok: true, value: undefined };
        },
      },
    } as unknown as LoamsDesktopApi,
  };
};

describe('graph page', () => {
  it('graph_empty_state', () => {
    const { api, opened } = desktop();
    render(<GraphPage desktop={api} flags={{ has: () => false }} />);
    expect(screen.getByText('Loams Graph is coming')).toBeTruthy();
    expect(screen.getByText(/GQL over loams\.graph\.v1, served by the loams engine/)).toBeTruthy();
    expect(screen.getByText('Not served')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Read the docs' }));
    expect(opened).toEqual(['https://loams.dev/docs']);
  });

  it('lights_up_when_the_instance_serves_graph', () => {
    const { api } = desktop();
    render(<GraphPage desktop={api} flags={{ has: (a) => a === 'loams.graph.v1' }} />);
    expect(screen.getByText('Available — editor coming in GR1a')).toBeTruthy();
    expect(screen.getByText('Served')).toBeTruthy();
    expect(screen.queryByText('Loams Graph is coming')).toBeNull();
  });
});
