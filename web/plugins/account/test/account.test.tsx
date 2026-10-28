// Sign-in, setup and consent on loams.auth.v1, ported from the original console
// (API2 Task 10).

import {
  createAdminTransport,
  stubRouter,
  stubSession,
  withConsole,
} from '@loams/admin-ui/testing';
import type { FlagsService } from '@loams/console-host';
import { auth } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { safeNext } from '../src/index.js';
import { ConsentPage, SetupPage, SignInPage } from '../src/pages.js';

afterEach(cleanup);

const flags: FlagsService = {
  edition: 'oss',
  instanceName: 'Acme Loams',
  serverVersion: '0.1.0-preview.1',
  features: {},
  apiVersions: ['loams.auth.v1'],
  has: () => true,
};

const CONSENT = {
  client_id: 'claude-code',
  redirect_uri: 'http://127.0.0.1:9999/cb',
  scope: 'query collections:read',
  audience: 'docs-search/development',
  state: 's',
  code_challenge: 'abc',
  code_challenge_method: 'S256',
};

describe('@loams/plugin-account', () => {
  it('sign_in_creates_a_session_and_returns_to_next', async () => {
    const sessions: auth.CreateSessionRequest[] = [];
    const transport = createAdminTransport({
      auth: {
        listProviders: () => ({
          password: true,
          totp: true,
          oidc: [{ id: 'keycloak', name: 'Acme SSO', kind: auth.OidcProviderKind.KEYCLOAK }],
        }),
        createSession: (req) => {
          sessions.push(req);
          return {};
        },
      },
    });
    const router = stubRouter('/sign-in', { next: '/teams' });
    const session = stubSession({ gate: 'signed-out', account: null });
    render(
      withConsole(
        <SignInPage
          router={router}
          session={session}
          flags={flags}
          server="https://loams.example"
        />,
        transport,
      ),
    );
    expect(screen.getByRole('heading', { name: 'Sign in to Acme Loams' })).toBeTruthy();
    const sso = (await screen.findByText(/Continue with Acme SSO/)).closest('a') as HTMLElement;
    expect(sso.getAttribute('href')).toBe(
      'https://loams.example/api/v1/auth/oidc/keycloak/start?next=%2Fteams',
    );
    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'priya@acme.dev' } });
    fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'correct horse' } });
    fireEvent.change(screen.getByLabelText('Two-factor code'), { target: { value: '123456' } });
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => expect(router.navigated).toEqual(['/teams']));
    expect(sessions[0]).toMatchObject({
      email: 'priya@acme.dev',
      password: 'correct horse',
      totpCode: '123456',
    });
    expect(session.refreshed).toBe(1);
  });

  it('only_follows_console_routes_after_sign_in', () => {
    expect(safeNext('/projects/docs')).toBe('/projects/docs');
    expect(safeNext('//evil.example')).toBe('/');
    expect(safeNext('https://evil.example')).toBe('/');
    expect(safeNext(undefined)).toBe('/');
  });

  it('setup_creates_the_org_and_refreshes_the_session', async () => {
    const setups: auth.CompleteSetupRequest[] = [];
    const transport = createAdminTransport({
      auth: {
        completeSetup: (req) => {
          setups.push(req);
          return {};
        },
      },
    });
    const router = stubRouter('/setup');
    const session = stubSession({ gate: 'setup', account: null });
    render(withConsole(<SetupPage router={router} session={session} />, transport));
    const fill = (label: string, value: string) =>
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    fill('Setup token', 'tok');
    fill('Organization name', 'Acme');
    fill('Your name', 'Priya');
    fill('Email', 'priya@acme.dev');
    fill('Password', 'short');
    fireEvent.click(screen.getByRole('button', { name: 'Create organization' }));
    expect(await screen.findByText('Use a password of at least 12 characters.')).toBeTruthy();
    fill('Password', 'a long enough password');
    fireEvent.click(screen.getByRole('button', { name: 'Create organization' }));
    await waitFor(() => expect(router.navigated).toEqual(['/']));
    expect(setups[0]).toMatchObject({
      setupToken: 'tok',
      orgName: 'Acme',
      email: 'priya@acme.dev',
    });
    expect(session.refreshed).toBe(1);
  });

  it('setup_needs_no_token_when_sign_in_is_off', async () => {
    const setups: auth.CompleteSetupRequest[] = [];
    const transport = createAdminTransport({
      auth: {
        completeSetup: (req) => {
          setups.push(req);
          return {};
        },
      },
    });
    const router = stubRouter('/setup');
    const session = stubSession({ gate: 'setup', account: null });
    render(
      withConsole(<SetupPage router={router} session={session} tokenNeeded={false} />, transport),
    );
    expect(screen.queryByLabelText('Setup token')).toBeNull();
    expect(screen.getByText(/needs no token/)).toBeTruthy();
    const fill = (label: string, value: string) =>
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    fill('Organization name', 'Acme');
    fill('Your name', 'Priya');
    fill('Email', 'priya@acme.dev');
    fill('Password', 'a long enough password');
    fireEvent.click(screen.getByRole('button', { name: 'Create organization' }));
    await waitFor(() => expect(router.navigated).toEqual(['/']));
    expect(setups[0]).toMatchObject({ setupToken: '', orgName: 'Acme' });
  });

  it('consent_decides_and_follows_the_redirect', async () => {
    const decisions: auth.DecideConsentRequest[] = [];
    const transport = createAdminTransport({
      auth: {
        decideConsent: (req) => {
          decisions.push(req);
          return { redirectTo: 'http://127.0.0.1:9999/cb?code=c&state=s' };
        },
      },
    });
    const went: string[] = [];
    render(
      withConsole(
        <ConsentPage
          router={stubRouter('/consent', CONSENT)}
          session={stubSession()}
          navigate={(url) => went.push(url)}
        />,
        transport,
      ),
    );
    expect(screen.getByRole('heading', { name: 'claude-code wants to act as you' })).toBeTruthy();
    expect(screen.getByText('docs-search/development')).toBeTruthy();
    expect(screen.getByText('collections:read')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
    await waitFor(() => expect(went).toEqual(['http://127.0.0.1:9999/cb?code=c&state=s']));
    expect(decisions[0]).toMatchObject({
      decision: auth.ConsentDecision.ALLOW,
      clientId: 'claude-code',
      codeChallenge: 'abc',
      codeChallengeMethod: 'S256',
    });
  });

  it('consent_without_pkce_is_incomplete', () => {
    const { code_challenge_method: _m, ...rest } = CONSENT;
    render(
      withConsole(<ConsentPage router={stubRouter('/consent', rest)} session={stubSession()} />),
    );
    expect(screen.getByRole('heading', { name: 'This link is incomplete' })).toBeTruthy();
  });
});
