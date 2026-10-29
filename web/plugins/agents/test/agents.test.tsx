// A project's agents, ported from the original console (API2 Task 10).

import {
  createAdminTransport,
  SEED,
  stubRouter,
  stubSession,
  withConsole,
} from '@loams/admin-ui/testing';
import { admin } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AgentPage } from '../src/agent.js';
import { AgentsPage } from '../src/agents.js';
import { exchangeSnippet } from '../src/index.js';

afterEach(cleanup);

describe('@loams/plugin-agents', () => {
  it('agents_page_lists_agents_with_status_and_policy', async () => {
    const router = stubRouter('/projects/docs-search/agents');
    render(
      withConsole(<AgentsPage project="docs-search" router={router} session={stubSession()} />),
    );
    expect(await screen.findByText('docs-qa')).toBeTruthy();
    expect(screen.getByText('Active')).toBeTruthy();
    expect(screen.getByText('Suspended')).toBeTruthy();
    expect(screen.getByText('10 min')).toBeTruthy();
    const link = screen.getByText('docs-qa').closest('a') as HTMLElement;
    expect(link.getAttribute('href')).toBe('#/projects/docs-search/agents/agt_docs_qa');
  });

  it('register_agent_sends_its_policy_and_owner', async () => {
    const created: admin.CreateAgentRequest[] = [];
    const transport = createAdminTransport({
      agents: {
        createAgent: (req) => {
          created.push(req);
          return { agent: { id: 'agt_new', name: req.name } };
        },
      },
    });
    render(
      withConsole(
        <AgentsPage project="docs-search" router={stubRouter()} session={stubSession()} />,
        transport,
      ),
    );
    fireEvent.click(await screen.findByRole('button', { name: /Register agent/ }));
    fireEvent.change(await screen.findByLabelText('Name'), { target: { value: 'Docs-Bot' } });
    // Wait for the environments, so the default (unprotected development) is picked.
    await screen.findByText('Development');
    fireEvent.click(
      screen.getAllByRole('button', { name: 'Register agent' }).at(-1) as HTMLElement,
    );
    await waitFor(() => expect(created).toHaveLength(1));
    expect(created[0]).toMatchObject({
      project: 'docs-search',
      name: 'docs-bot',
      maxTokenTtlS: 900,
      owner: { kind: admin.SubjectKind.USER, id: SEED.user.id },
    });
    expect(created[0]?.policy[0]).toMatchObject({ environment: 'development', actions: ['query'] });
    expect(await screen.findByText('Registered docs-bot')).toBeTruthy();
  });

  it('agent_page_shows_trust_tokens_and_suspends', async () => {
    const suspended: string[] = [];
    const transport = createAdminTransport({
      agents: {
        suspendAgent: (req) => {
          suspended.push(req.agent);
          return { agent: { id: req.agent, status: admin.AgentStatus.SUSPENDED } };
        },
      },
    });
    const router = stubRouter('/projects/docs-search/agents/agt_docs_qa');
    render(
      withConsole(
        <AgentPage
          project="docs-search"
          agent="agt_docs_qa"
          router={router}
          session={stubSession()}
          server="https://loams.example"
        />,
        transport,
      ),
    );
    expect(await screen.findByRole('heading', { name: /docs-qa/ })).toBeTruthy();
    expect(await screen.findByText('Docs site on EKS')).toBeTruthy();
    expect(await screen.findByText('tok_a1b0')).toBeTruthy();
    expect(screen.getByText(/https:\/\/loams.example\/api\/v1\/oauth\/token/)).toBeTruthy();
    expect(router.trail()?.map((c) => c.label)).toEqual([
      'Acme Robotics',
      'Docs search',
      'Agents',
      'docs-qa',
    ]);
    fireEvent.click(screen.getByRole('button', { name: /Suspend/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Suspend and revoke tokens' }));
    await waitFor(() => expect(suspended).toEqual(['agt_docs_qa']));
    expect(await screen.findByText(/Every token was revoked/)).toBeTruthy();
  });

  it('exchange_snippet_names_the_server_and_the_first_rule', () => {
    const snippet = exchangeSnippet(
      'https://loams.example',
      'docs-search',
      SEED.agents[0]?.policy[0],
    );
    expect(snippet).toContain('curl -X POST https://loams.example/api/v1/oauth/token');
    expect(snippet).toContain('audience=docs-search/production');
    expect(snippet).toContain('scope="query"');
  });
});
