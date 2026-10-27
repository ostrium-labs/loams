// Settings › Billing › Payment (design 63 §18): the payment state and a link to the billing
// provider's hosted page (the Stripe Customer Portal). Loams never shows a card form and never
// handles card data; the state comes back from the provider's webhooks (§13).

import { Badge, buttonClass, Card, Notice, Stat, Stats } from '@loams/ui';
import { Loaded, useLoad } from './load.js';
import type { BillingClient, PaymentView } from './types.js';
import { PAYMENT_LABEL } from './usage.js';

const PROVIDER: Record<string, string> = { stripe: 'Stripe', lago: 'Lago' };

/** Only an https link to the provider is followed; anything else is not shown. */
export function safePortalUrl(view: PaymentView): string | undefined {
  if (!view.portalUrl) return undefined;
  try {
    const url = new URL(view.portalUrl);
    return url.protocol === 'https:' ? url.toString() : undefined;
  } catch {
    return undefined;
  }
}

export function PaymentPage({ client }: { client: BillingClient }) {
  const [load] = useLoad(() => client.payment(), [client]);
  return (
    <Loaded load={load} what="payment">
      {(view) => {
        const provider = view.provider ? (PROVIDER[view.provider] ?? view.provider) : undefined;
        const url = safePortalUrl(view);
        return (
          <div className="grid gap-4">
            <Stats>
              <Stat
                label="Payment state"
                value={
                  <span className="inline-flex items-center gap-2">
                    {PAYMENT_LABEL[view.state]}
                    <Badge variant={view.state === 'none' ? 'warning' : 'success'}>
                      {view.state}
                    </Badge>
                  </span>
                }
                detail={view.methodSummary}
              />
              <Stat label="Provider" value={provider ?? 'None configured'} />
            </Stats>
            {view.state === 'none' && (
              <Notice tone="warn" title="Usage stops at the plan's allowance">
                Without a payment method, metered features stop when their allowance is used up. Add
                a card to buy credit or to turn on overage.
              </Notice>
            )}
            {view.state === 'invoice' && (
              <Notice title="Billed by contract">
                This organisation is invoiced under a contract; an operator manages its payment
                terms.
              </Notice>
            )}
            <Card title="Payment method">
              {url && provider ? (
                <div className="grid gap-2">
                  <p className="text-muted">
                    Cards and invoices are managed on {provider}'s own page. Loams never sees your
                    card details.
                  </p>
                  {view.canEdit ? (
                    <a
                      className={buttonClass({ variant: 'primary' })}
                      href={url}
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {view.state === 'none' ? 'Add a card' : 'Manage payment'} on {provider}
                    </a>
                  ) : (
                    <p className="text-small text-muted">
                      Managing the payment method needs the billing admin role.
                    </p>
                  )}
                </div>
              ) : (
                <p className="text-muted">
                  No billing provider is configured for this deployment. An operator sets the
                  payment state.
                </p>
              )}
            </Card>
          </div>
        );
      }}
    </Loaded>
  );
}
