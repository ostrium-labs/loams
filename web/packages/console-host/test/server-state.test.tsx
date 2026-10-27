import { useTransport } from '@connectrpc/connect-query';
import { useQueryClient } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { createMockTransport, createQueryClient, ServerStateProvider } from '../src/testing';

afterEach(cleanup);

describe('ServerStateProvider', () => {
  it('gives the tree one query client and the console transport', () => {
    const client = createQueryClient();
    const transport = createMockTransport();
    let seen: unknown[] = [];
    function Probe() {
      seen = [useQueryClient(), useTransport()];
      return <p>ok</p>;
    }
    render(
      <ServerStateProvider client={client} transport={transport}>
        <Probe />
      </ServerStateProvider>,
    );
    expect(screen.getByText('ok')).toBeTruthy();
    expect(seen).toEqual([client, transport]);
  });
});
