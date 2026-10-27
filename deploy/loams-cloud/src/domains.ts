// Email-domain lists (design 78 §25.4, D1753; pins in plan R-LC2a-10).
//
// - The disposable list: disposable-email-domains/disposable-email-domains at
//   8c05972c (CC0-1.0), vendored as policy/disposable_email_blocklist.conf.
// - The free-mail list (Kikobeats/free-email-domains, MIT) is not vendored yet:
//   its free-mail source is HubSpot's unlicensed CSV, which waits on counsel
//   (Q1276, R-LC2a-10). Until then the email-code rule uses `block` alone.
// - policy/email-domains.toml: our `block` and `allow` overrides.

import { parseToml } from "./toml.ts";

export interface DomainLists {
  disposable: ReadonlySet<string>;
  freeMail: ReadonlySet<string>;
  block: readonly string[];
  allow: ReadonlySet<string>;
}

export function parseList(text: string): Set<string> {
  const out = new Set<string>();
  for (const raw of text.split("\n")) {
    const d = raw.replace(/#.*/, "").trim().toLowerCase();
    if (d) out.add(d);
  }
  return out;
}

export function loadDomainLists(files: { disposable: string; overrides: string; freeMail?: string }): DomainLists {
  const o = parseToml(files.overrides, "email-domains.toml");
  const block = (o.block ?? []) as string[];
  const allow = (o.allow ?? []) as string[];
  if (!Array.isArray(block) || !Array.isArray(allow)) throw new Error("email-domains.toml: block and allow are arrays");
  const both = allow.filter((d) => block.includes(d));
  if (both.length) throw new Error(`email-domains.toml: in both block and allow: ${both.join(", ")}`);
  return {
    disposable: parseList(files.disposable),
    freeMail: parseList(files.freeMail ?? ""),
    block: block.map((d) => d.toLowerCase()),
    allow: new Set(allow.map((d) => d.toLowerCase())),
  };
}

/** A `name.*` pattern matches `name.` plus a public suffix of one or two labels. */
function matchesPattern(domain: string, pattern: string): boolean {
  if (!pattern.endsWith(".*")) return domain === pattern;
  const stem = pattern.slice(0, -1); // "yahoo."
  if (!domain.startsWith(stem)) return false;
  return /^[a-z]{2,63}(\.[a-z]{2,3})?$/.test(domain.slice(stem.length));
}

/** The domain and each parent (mail.example.co.uk, example.co.uk, co.uk). */
function selfAndParents(domain: string): string[] {
  const labels = domain.split(".");
  const out: string[] = [];
  for (let i = 0; i < labels.length - 1; i++) out.push(labels.slice(i).join("."));
  return out;
}

export function isAllowed(lists: DomainLists, domain: string): boolean {
  return lists.allow.has(domain);
}

/** The waitlist rule (D1753): disposable domains are refused; free mail is fine. */
export function isDisposable(lists: DomainLists, domain: string): boolean {
  if (isAllowed(lists, domain)) return false;
  return selfAndParents(domain).some((d) => lists.disposable.has(d));
}

/** The email-code rule (D1751, D1753): only work domains. MX is checked separately. */
export function emailCodeDomainRefusal(lists: DomainLists, domain: string): null | "blocked" | "free_mail" | "disposable" {
  if (lists.block.some((p) => matchesPattern(domain, p))) return "blocked";
  if (isAllowed(lists, domain)) return null;
  if (isDisposable(lists, domain)) return "disposable";
  if (selfAndParents(domain).some((d) => lists.freeMail.has(d))) return "free_mail";
  return null;
}

/** Whether a domain can receive mail: one MX lookup over DNS over HTTPS. */
export interface Dns {
  hasMx(domain: string): Promise<boolean>;
}

/**
 * Cloudflare's DNS-over-HTTPS JSON API. A domain with no MX, or with a null MX
 * (RFC 7505, "0 ."), can't receive mail. A lookup that fails is treated as "has
 * MX" only when `failOpen` is set; the waitlist sets it, because refusing on a
 * resolver hiccup would turn real people away silently.
 */
export class DohDns implements Dns {
  private readonly fetchFn: typeof fetch;
  private readonly failOpen: boolean;
  constructor(opts: { fetch?: typeof fetch; failOpen?: boolean } = {}) {
    this.fetchFn = opts.fetch ?? fetch;
    this.failOpen = opts.failOpen ?? false;
  }

  async hasMx(domain: string): Promise<boolean> {
    try {
      const r = await this.fetchFn(`https://cloudflare-dns.com/dns-query?name=${encodeURIComponent(domain)}&type=MX`, {
        headers: { accept: "application/dns-json" },
        signal: AbortSignal.timeout(3000),
      });
      if (!r.ok) return this.failOpen;
      const body = (await r.json()) as { Status: number; Answer?: { type: number; data: string }[] };
      if (body.Status === 3) return false; // NXDOMAIN
      if (body.Status !== 0) return this.failOpen;
      const mx = (body.Answer ?? []).filter((a) => a.type === 15);
      return mx.some((a) => !/^0\s+\.?$/.test(a.data.trim()));
    } catch {
      return this.failOpen;
    }
  }
}
