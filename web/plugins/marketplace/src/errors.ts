// agentd's marketplace errors in plain words (plan MK1 Task 4b, ruling 6). The reason
// is in the Connect error's `loams.errors.v1.ErrorInfo`; every other error keeps the
// daemon's own sentence.

import { ConnectError } from '@connectrpc/connect';
import { ErrorInfoSchema } from '@loams/proto/errors';
import { plainError } from '@loams/ui';

export type MarketplaceReason =
  | 'plan_changed'
  | 'consent_required'
  | 'listing_revoked'
  | 'untrusted_listing';

export interface MarketplaceError {
  reason?: MarketplaceReason | string;
  /** What the page says. */
  message: string;
}

const SENTENCES: Record<MarketplaceReason, string> = {
  plan_changed:
    'The listing changed after this plan was made, so nothing was installed. Read the new plan and confirm it again.',
  consent_required:
    'This version asks for more than the installed one, so it needs your consent again. Read its plan and confirm it.',
  listing_revoked:
    'This version has been revoked or yanked, so it cannot be installed. Choose another version or wait for a fixed one.',
  untrusted_listing:
    'agentd refused this listing: its index signature, integrity or provenance did not check out, or it runs code at install. Nothing was installed.',
};

export function marketplaceError(e: unknown): MarketplaceError {
  const err = ConnectError.from(e);
  const reason = err.findDetails(ErrorInfoSchema)[0]?.reason || undefined;
  const known = reason && reason in SENTENCES ? SENTENCES[reason as MarketplaceReason] : undefined;
  return {
    ...(reason ? { reason } : {}),
    message: known ?? plainError(err.rawMessage || err.message),
  };
}
