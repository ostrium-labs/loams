import { useMutation, useQuery } from '@connectrpc/connect-query';
import {
  errorText,
  Form,
  iso,
  LoadError,
  Loading,
  Page,
  PageHead,
  useAccount,
  useNotify,
  useTrail,
} from '@loams/admin-ui';
import type { FlagsService, RouterService, SessionService } from '@loams/console-host';
import { admin, auth } from '@loams/proto';
import { Badge, Button, Card, Checkbox, Field, formatDate, Input, StatusTag } from '@loams/ui';
import { useState } from 'react';

const EDITION: Record<string, string> = {
  oss: 'Open source',
  cloud: 'Loams Cloud',
  byoc: 'BYOC',
};

const OIDC_KIND: Record<number, string> = {
  [auth.OidcProviderKind.KEYCLOAK]: 'keycloak',
  [auth.OidcProviderKind.OKTA]: 'okta',
  [auth.OidcProviderKind.ENTRA]: 'entra',
  [auth.OidcProviderKind.GOOGLE]: 'google',
  [auth.OidcProviderKind.AUTHENTIK]: 'authentik',
  [auth.OidcProviderKind.DEX]: 'dex',
  [auth.OidcProviderKind.GENERIC]: 'generic',
  [auth.OidcProviderKind.ZITADEL]: 'zitadel',
};

function OrgForm({ org, onSaved }: { org: admin.Org; onSaved: () => void }) {
  const [name, setName] = useState(org.name);
  const [twoFactor, setTwoFactor] = useState(org.requireTwoFactor);
  const [domains, setDomains] = useState(org.allowedDomains.join(', '));
  const update = useMutation(admin.OrgService.method.updateOrg);
  const notify = useNotify();
  return (
    <Form
      onSubmit={async (ev) => {
        ev.preventDefault();
        try {
          await update.mutateAsync({
            org: {
              id: org.id,
              name: name.trim(),
              requireTwoFactor: twoFactor,
              allowedDomains: domains
                .split(',')
                .map((d) => d.trim())
                .filter(Boolean),
            },
            updateMask: { paths: ['name', 'require_two_factor', 'allowed_domains'] },
          });
          notify('Saved');
          onSaved();
        } catch (e) {
          notify(errorText(e), 'danger');
        }
      }}
    >
      <Field label="Name">
        {(p) => <Input {...p} value={name} onChange={(e) => setName(e.target.value)} />}
      </Field>
      <Field
        label="Allowed email domains"
        hint="Single sign-on creates members only for these domains."
      >
        {(p) => <Input {...p} value={domains} onChange={(e) => setDomains(e.target.value)} />}
      </Field>
      <Checkbox
        label="Require two-factor sign-in for password accounts"
        checked={twoFactor}
        onChange={(e) => setTwoFactor(e.target.checked)}
      />
      <div>
        <Button variant="primary" type="submit" disabled={update.isPending}>
          Save changes
        </Button>
      </div>
    </Form>
  );
}

const onOff = (on: boolean | undefined, off: 'Off' | 'Planned' = 'Off') =>
  on ? (
    <StatusTag status="done">On</StatusTag>
  ) : (
    <StatusTag status={off === 'Planned' ? 'planned' : 'neutral'}>{off}</StatusTag>
  );

export function SettingsPage({
  router,
  session,
  flags,
}: {
  router: RouterService;
  session: SessionService;
  flags: FlagsService;
}) {
  const account = useAccount(session);
  useTrail(router, [
    { label: account?.org?.name ?? 'Organization', href: '/' },
    { label: 'Settings' },
  ]);
  const org = useQuery(admin.OrgService.method.getOrg, {});
  const providers = useQuery(auth.AuthService.method.listProviders, {});
  const current = org.data?.org ?? account?.org;
  const s = providers.data;
  const created = iso(current?.createdAt);
  return (
    <Page>
      <PageHead eyebrow="Organization" title="Settings" />
      <div className="grid-2">
        <Card title="Organization">
          {current ? (
            <OrgForm
              key={`${current.id}:${current.name}`}
              org={current}
              onSaved={() => {
                void org.refetch();
                void session.refresh?.();
              }}
            />
          ) : org.error ? (
            <LoadError error={org.error} retry={() => void org.refetch()} />
          ) : (
            <Loading />
          )}
        </Card>
        <Card title="Sign-in methods">
          {providers.error ? (
            <LoadError error={providers.error} retry={() => void providers.refetch()} />
          ) : (
            <ul className="methods">
              <li>
                <span>Email and password</span>
                {onOff(s?.password)}
              </li>
              <li>
                <span>Two-factor codes (TOTP)</span>
                {onOff(s?.totp)}
              </li>
              {(s?.oidc ?? []).map((o) => (
                <li key={o.id}>
                  <span>
                    {o.name} <Badge>OIDC, {OIDC_KIND[o.kind] ?? 'generic'}</Badge>
                  </span>
                  <StatusTag status="done">On</StatusTag>
                </li>
              ))}
              <li>
                <span>Passkeys</span>
                {onOff(s?.passkeys, 'Planned')}
              </li>
            </ul>
          )}
          <p>
            SAML and SCIM come through your identity provider: put Zitadel, Keycloak or Dex in front
            and connect it over OIDC.
          </p>
        </Card>
      </div>
      <Card title="This install" style={{ marginTop: 16 }}>
        <dl className="facts">
          <div>
            <dt>Edition</dt>
            <dd>{EDITION[flags.edition] ?? 'Open source'}</dd>
          </div>
          <div>
            <dt>Version</dt>
            <dd>{flags.serverVersion || '…'}</dd>
          </div>
          <div>
            <dt>Org id</dt>
            <dd>
              <code>{current?.id}</code>
            </dd>
          </div>
          <div>
            <dt>Created</dt>
            <dd>{created ? formatDate(created) : '…'}</dd>
          </div>
        </dl>
      </Card>
    </Page>
  );
}
