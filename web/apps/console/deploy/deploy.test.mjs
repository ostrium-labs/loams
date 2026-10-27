// node --test apps/console/deploy/*.test.mjs (CI's web job runs it).
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { after, describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  DEFAULT_SCOPE,
  loadRuntimeConfig,
  offeredModes,
  parseRuntimeConfig,
} from '../src/runtime-config.ts';
import { agentdOrigins, hostedConfig } from './config.mjs';
import { assetUrls, isJavaScript, smoke } from './smoke.mjs';
import {
  checkBase,
  checkVercelCsp,
  connectSources,
  contentSecurityPolicy,
  headersFile,
  injectCsp,
  inlineScriptHashes,
  redirectsFile,
  stage,
  TAILNET,
} from './stage.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const console_ = join(here, '..');

const THEME = "\n      try { document.documentElement.classList.add('dark'); } catch {}\n    ";
const INDEX = `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <link rel="icon" href="/ui/favicon.svg" type="image/svg+xml" />
    <script>${THEME}</script>
    <script type="module" crossorigin src="/ui/assets/index-abc.js"></script>
    <link rel="stylesheet" crossorigin href="/ui/assets/index-abc.css">
  </head>
  <body><div id="root"></div></body>
</html>
`;

const CORDIS = INDEX.replace(/<script>[\s\S]*?<\/script>\n\s*/, '').replace(
  'index-abc',
  'cordis-abc',
);
const AUTH = { LOAMS_AUTH_ISSUER: 'https://auth.loams.dev', LOAMS_AUTH_CLIENT_ID: 'loams-console' };

/** A console build: today's console, the cordis console, config and assets. */
function fakeDist(dir) {
  mkdirSync(join(dir, 'assets'), { recursive: true });
  writeFileSync(join(dir, 'index.html'), INDEX);
  writeFileSync(join(dir, 'cordis.html'), injectCsp(CORDIS, "connect-src 'self'"));
  writeFileSync(join(dir, 'config.json'), '{}\n');
  writeFileSync(join(dir, 'assets', 'index-abc.js'), 'export {};\n');
  writeFileSync(join(dir, 'assets', 'index-abc.js.map'), '{}');
  return dir;
}

const vercel = JSON.parse(readFileSync(join(console_, 'vercel.json'), 'utf8'));

describe('wrangler.jsonc', () => {
  const text = readFileSync(join(console_, 'wrangler.jsonc'), 'utf8');
  const config = JSON.parse(text.replace(/^\s*\/\/.*$/gm, ''));

  test('is the assets-only loams-console Worker with no console.loams.dev route', () => {
    assert.equal(config.name, 'loams-console');
    assert.equal(config.main, undefined, 'no Worker code: free tier, no CPU budget used');
    assert.equal(config.routes, undefined, 'no custom-domain route (owner, 2026-10-24)');
    assert.equal(config.workers_dev, true);
    assert.ok(!text.includes('"console.loams.dev"'), 'no console.loams.dev pattern');
  });

  test('serves the staged directory with an SPA fallback', () => {
    assert.equal(config.assets.directory, './dist-cloudflare');
    assert.equal(config.assets.not_found_handling, 'single-page-application');
    assert.equal(config.assets.html_handling, 'none', 'preserve exact sandbox paths and headers');
  });

  test('a BYOC deploy (--env byoc) stays on workers.dev with no routes', () => {
    assert.deepEqual(config.env.byoc.routes, []);
    assert.equal(config.env.byoc.workers_dev, true);
    assert.equal(config.env.byoc.main, undefined);
  });
});

describe('vercel.json', () => {
  const all = vercel.headers.find((h) => h.source === '/(.*)').headers;
  const header = (key) => all.find((h) => h.key === key)?.value;

  test('builds from the workspace and publishes the staged Vercel output', () => {
    assert.equal(vercel.framework, null);
    assert.match(vercel.installCommand, /pnpm install --frozen-lockfile/);
    assert.match(vercel.buildCommand, /stage\.mjs --target vercel$/);
    assert.equal(vercel.outputDirectory, 'dist-vercel');
  });

  test('falls back to the console for unknown paths, never for assets', () => {
    const [rule] = vercel.rewrites;
    assert.equal(rule.destination, '/index.html');
    const re = new RegExp(`^${rule.source.replace('(.*)', '.*')}$`);
    assert.ok(re.test('/projects/p1'));
    assert.ok(!re.test('/ui/assets/index-abc.js'));
  });

  test('sends a CSP without inline or eval scripts that admits the tailnet and the issuer', () => {
    const csp = header('Content-Security-Policy');
    assert.match(csp, /script-src 'self' 'wasm-unsafe-eval';/);
    assert.doesNotMatch(csp, /'unsafe-eval'|script-src[^;]*unsafe-inline/);
    assert.match(csp, /connect-src 'self' https:\/\/\*\.ts\.net https:\/\/auth\.loams\.dev;/);
    assert.match(csp, /object-src 'none'/);
    for (const key of [
      'X-Content-Type-Options',
      'Referrer-Policy',
      'Strict-Transport-Security',
      'X-Frame-Options',
    ]) {
      assert.ok(header(key), key);
    }
  });

  test('the sandbox frame keeps its own policy and config.json is never cached', () => {
    const frame = vercel.headers.find((h) => h.source === '/ui/sandbox/frame.html');
    assert.match(frame.headers[0].value, /connect-src 'none'.*sandbox allow-scripts/);
    const config = vercel.headers.find((h) => h.source === '/(ui/)?config.json');
    assert.equal(config.headers[0].value, 'no-store');
  });

  test('the stage step refuses a config its static CSP would block', () => {
    checkVercelCsp(vercel, connectSources(hostedConfig(AUTH)));
    assert.throws(
      () =>
        checkVercelCsp(
          vercel,
          connectSources(hostedConfig({ ...AUTH, LOAMS_AUTH_ISSUER: 'https://id.example' })),
        ),
      /lacks https:\/\/id\.example/,
    );
  });
});

describe('hosted config', () => {
  test('writes the engine, default mode, issuer and client id from the environment', () => {
    assert.deepEqual(
      hostedConfig({
        ...AUTH,
        LOAMS_ENGINE_URL: 'https://loams.tail1.ts.net/x',
        LOAMS_AUTH_SCOPE: 'openid',
      }),
      {
        hosted: true,
        engineUrl: 'https://loams.tail1.ts.net',
        // tailscale is the default mode; the client stays so oidc can be picked.
        authMode: 'tailscale',
        authIssuer: 'https://auth.loams.dev',
        authClientId: 'loams-console',
        authScope: 'openid',
      },
    );
    // auth.loams.dev is the default issuer.
    assert.deepEqual(
      hostedConfig({ LOAMS_AUTH_MODE: 'OIDC', LOAMS_AUTH_CLIENT_ID: 'loams-console' }),
      {
        hosted: true,
        authMode: 'oidc',
        authIssuer: 'https://auth.loams.dev',
        authClientId: 'loams-console',
      },
    );
    // Without a client: tailscale and apikey only.
    assert.deepEqual(hostedConfig({}), { hosted: true, authMode: 'tailscale' });
    assert.deepEqual(hostedConfig({ LOAMS_AUTH_MODE: 'apikey' }), {
      hosted: true,
      authMode: 'apikey',
    });
    assert.equal(
      hostedConfig({ LOAMS_CONSOLE_SERVER: 'https://a.example' }).engineUrl,
      'https://a.example',
    );
    assert.equal(
      hostedConfig({ ...AUTH, LOAMS_AUTH_ISSUER: 'https://id.example/realms/loams/' }).authIssuer,
      'https://id.example/realms/loams',
    );
  });

  test('narrows the offered modes with LOAMS_AUTH_MODES', () => {
    // A deploy that has tested only Tailscale, even though it has a client.
    assert.deepEqual(hostedConfig({ ...AUTH, LOAMS_AUTH_MODES: 'tailscale' }), {
      hosted: true,
      authMode: 'tailscale',
      authModes: ['tailscale'],
      authIssuer: 'https://auth.loams.dev',
      authClientId: 'loams-console',
    });
    // Separators and order do not matter: the console's order comes back.
    assert.deepEqual(
      hostedConfig({ LOAMS_AUTH_MODES: 'apikey, tailscale', LOAMS_AUTH_MODE: 'apikey' }).authModes,
      ['tailscale', 'apikey'],
    );
    assert.throws(() => hostedConfig({ LOAMS_AUTH_MODES: 'basic' }), /LOAMS_AUTH_MODES/);
    assert.throws(
      () => hostedConfig({ LOAMS_AUTH_MODE: 'apikey', LOAMS_AUTH_MODES: 'tailscale' }),
      /must include the default LOAMS_AUTH_MODE/,
    );
    assert.throws(() => hostedConfig({ LOAMS_AUTH_MODES: 'tailscale,oidc' }), /oidc/);
  });

  test('refuses plain http off loopback, oidc without a client id, and an unknown mode', () => {
    assert.throws(() => hostedConfig({ LOAMS_ENGINE_URL: 'http://loams.tail1.ts.net' }), /HTTPS/);
    assert.throws(() => hostedConfig({ LOAMS_ENGINE_URL: 'not a url' }), /HTTPS/);
    assert.equal(
      hostedConfig({ LOAMS_ENGINE_URL: 'http://127.0.0.1:8080' }).engineUrl,
      'http://127.0.0.1:8080',
    );
    assert.throws(() => hostedConfig({ LOAMS_AUTH_MODE: 'oidc' }), /LOAMS_AUTH_CLIENT_ID/);
    assert.throws(
      () => hostedConfig({ LOAMS_AUTH_ISSUER: 'https://a.example' }),
      /without LOAMS_AUTH_CLIENT_ID/,
    );
    assert.throws(() => hostedConfig({ ...AUTH, LOAMS_AUTH_MODE: 'basic' }), /LOAMS_AUTH_MODE/);
    // console.loams.dev requires the client, whatever the default mode.
    assert.throws(
      () => hostedConfig({}, { requireAuth: true }),
      /this deploy needs LOAMS_AUTH_CLIENT_ID/,
    );
    assert.equal(hostedConfig(AUTH, { requireAuth: true }).authClientId, 'loams-console');
  });
});

describe('stage', () => {
  test('hashes inline scripts and skips external ones', () => {
    const want = `'sha256-${createHash('sha256').update(THEME).digest('base64')}'`;
    assert.deepEqual(inlineScriptHashes(INDEX), [want]);
  });

  test('the CSP allows the inline hash and the given sources, and nothing remote otherwise', () => {
    const csp = contentSecurityPolicy({
      scriptHashes: ["'sha256-x'"],
      connect: connectSources({
        engineUrl: 'https://loams.example.com/path',
        authIssuer: 'https://auth.loams.dev/oidc',
      }),
    });
    assert.match(csp, /script-src 'self' 'wasm-unsafe-eval' 'sha256-x'(;|$)/);
    assert.match(
      csp,
      /connect-src 'self' https:\/\/\*\.ts\.net https:\/\/loams\.example\.com https:\/\/auth\.loams\.dev(;|$)/,
    );
    assert.doesNotMatch(csp, /'unsafe-eval'/);
    assert.match(contentSecurityPolicy(), /connect-src 'self'(;|$)/);
    assert.deepEqual(connectSources(), ["'self'", TAILNET]);
  });

  test('hashes inline scripts with whitespace or attributes in their closing tag', () => {
    const want = `'sha256-${createHash('sha256').update('window.theme = "dark";').digest('base64')}'`;
    for (const end of ['</script \n>', '</script\t\n bar>', '</script/>']) {
      assert.deepEqual(inlineScriptHashes(`<script>window.theme = "dark";${end}`), [want]);
    }
  });

  test('injects the CSP once, after the charset', () => {
    const once = injectCsp(INDEX, "default-src 'self'");
    assert.match(once, /<meta charset="utf-8" \/>\n {4}<meta http-equiv="Content-Security-Policy"/);
    assert.equal(injectCsp(once, 'other'), once);
  });

  test('rejects a build made for another base path', () => {
    assert.throws(
      () => checkBase(INDEX.replaceAll('/ui/assets/', '/assets/')),
      /expected \/ui\/assets/,
    );
    checkBase(INDEX);
  });

  test('headers cache hashed assets forever and config.json never', () => {
    const h = headersFile();
    assert.match(h, /\/ui\/assets\/\*\n {2}Cache-Control: public, max-age=31536000, immutable/);
    assert.match(h, /\/ui\/config\.json\n {2}Cache-Control: no-store/);
    assert.match(h, /X-Content-Type-Options: nosniff/);
    assert.equal(redirectsFile(), '/ui/ / 302\n/ui/index.html / 302\n');
  });

  const tmp = join(here, '.test-tmp');
  after(() => rmSync(tmp, { recursive: true, force: true }));

  test('refuses a bad config before altering staged files', () => {
    const dist = fakeDist(join(tmp, 'invalid-dist'));
    const out = join(tmp, 'invalid-out');
    mkdirSync(out, { recursive: true });
    writeFileSync(join(out, 'sentinel'), 'keep');
    for (const env of [
      { LOAMS_ENGINE_URL: 'http://loams.example.com' },
      { LOAMS_ENGINE_URL: 'javascript:alert(1)' },
      { LOAMS_ENGINE_URL: 'not a url' },
      { LOAMS_AUTH_ISSUER: 'https://auth.loams.dev' },
      { LOAMS_AUTH_MODE: 'password' },
    ]) {
      assert.throws(() => stage({ dist, out, env }), /HTTPS|LOAMS_AUTH_CLIENT_ID|LOAMS_AUTH_MODE/);
      assert.equal(readFileSync(join(out, 'sentinel'), 'utf8'), 'keep');
    }
    assert.throws(() => stage({ dist, out, env: {}, requireAuth: true }), /LOAMS_AUTH_CLIENT_ID/);
    assert.throws(
      () =>
        stage({
          dist,
          out,
          env: { ...AUTH, LOAMS_AUTH_ISSUER: 'https://id.example' },
          target: 'vercel',
          vercelJson: vercel,
        }),
      /vercel\.json/,
    );
    assert.equal(readFileSync(join(out, 'sentinel'), 'utf8'), 'keep');
  });

  test('serves the cordis console at the root with the runtime config and its CSP', () => {
    const dist = fakeDist(join(tmp, 'dist'));
    const out = join(tmp, 'out');
    stage({ dist, out, env: { ...AUTH, LOAMS_ENGINE_URL: 'https://loams.tail1.ts.net/' } });

    assert.ok(existsSync(join(out, 'ui', 'assets', 'index-abc.js')));
    const root = readFileSync(join(out, 'index.html'), 'utf8');
    assert.equal(root, readFileSync(join(out, 'ui', 'cordis.html'), 'utf8'));
    assert.match(root, /cordis-abc\.js/);
    assert.equal(root.match(/http-equiv="Content-Security-Policy"/g)?.length, 1);
    assert.match(
      root,
      /connect-src 'self' https:\/\/\*\.ts\.net https:\/\/loams\.tail1\.ts\.net https:\/\/auth\.loams\.dev/,
    );
    assert.match(
      root,
      /script-src 'self' 'wasm-unsafe-eval';/,
      'the cordis console has no inline script',
    );
    const want = {
      hosted: true,
      engineUrl: 'https://loams.tail1.ts.net',
      authMode: 'tailscale',
      authIssuer: 'https://auth.loams.dev',
      authClientId: 'loams-console',
    };
    assert.deepEqual(JSON.parse(readFileSync(join(out, 'config.json'), 'utf8')), want);
    assert.deepEqual(JSON.parse(readFileSync(join(out, 'ui', 'config.json'), 'utf8')), want);
    assert.equal(readFileSync(join(out, '.assetsignore'), 'utf8'), '*.map\n');
    assert.ok(existsSync(join(out, '_headers')));
    assert.equal(readFileSync(join(out, '_redirects'), 'utf8'), redirectsFile());

    // The sandbox frame keeps its own response policy.
    const headers = readFileSync(join(out, '_headers'), 'utf8');
    const sandbox = headers.split('/ui/sandbox/frame.html\n')[1];
    assert.ok(sandbox, 'sandbox frame needs its own response policy');
    const frame = readFileSync(join(console_, '../../plugins/sandbox/static/frame.html'), 'utf8');
    const frameCsp = /Content-Security-Policy"\s+content="([^"]+)"/.exec(frame)?.[1];
    assert.ok(frameCsp);
    assert.ok(sandbox.includes(`Content-Security-Policy: ${frameCsp}; sandbox allow-scripts`));

    // Without LOAMS_ENGINE_URL the console asks for the engine on first run.
    stage({ dist, out, env: { LOAMS_AUTH_MODE: 'tailscale' } });
    assert.deepEqual(JSON.parse(readFileSync(join(out, 'config.json'), 'utf8')), {
      hosted: true,
      authMode: 'tailscale',
    });
  });

  // SD1 Task 30, ruling T26-9: Studio on the web calls the remote agentd hosts and frames
  // their previews, which a CSP of `connect-src`/`frame-src 'self'` blocked.
  test('LOAMS_AGENTD_ORIGINS joins connect-src and frame-src, origins only', () => {
    const dist = fakeDist(join(tmp, 'agentd-dist'));
    const out = join(tmp, 'agentd-out');
    stage({
      dist,
      out,
      env: {
        ...AUTH,
        LOAMS_AGENTD_ORIGINS: 'https://dev-box.example.com:8443, https://vm.example.com/',
      },
    });
    const root = readFileSync(join(out, 'index.html'), 'utf8');
    assert.match(
      root,
      /connect-src 'self' https:\/\/\*\.ts\.net https:\/\/auth\.loams\.dev https:\/\/dev-box\.example\.com:8443 https:\/\/vm\.example\.com;/,
    );
    assert.match(
      root,
      /frame-src 'self' https:\/\/dev-box\.example\.com:8443 https:\/\/vm\.example\.com;/,
    );
    assert.deepEqual(agentdOrigins({}), []);
    for (const bad of [
      'https://*.example.com',
      'http://dev-box.example.com',
      'https://dev-box.example.com/agentd',
      'https://user@dev-box.example.com',
      'javascript:alert(1)',
      'dev-box.example.com',
    ]) {
      assert.throws(
        () => agentdOrigins({ LOAMS_AGENTD_ORIGINS: bad }),
        /LOAMS_AGENTD_ORIGINS/,
        bad,
      );
    }
    assert.deepEqual(agentdOrigins({ LOAMS_AGENTD_ORIGINS: 'http://localhost:8443' }), [
      'http://localhost:8443',
    ]);
    // Vercel's static CSP must name them too.
    assert.throws(
      () =>
        stage({
          dist,
          out: join(tmp, 'agentd-vercel'),
          env: { ...AUTH, LOAMS_AGENTD_ORIGINS: 'https://dev-box.example.com:8443' },
          target: 'vercel',
          vercelJson: vercel,
        }),
      /connect-src lacks https:\/\/dev-box\.example\.com:8443/,
    );
    assert.throws(
      () =>
        checkVercelCsp(
          {
            headers: [
              {
                source: '/(.*)',
                headers: [
                  {
                    key: 'Content-Security-Policy',
                    value: "connect-src 'self' https://a.example; frame-src 'self'",
                  },
                ],
              },
            ],
          },
          ["'self'", 'https://a.example'],
          ['https://a.example'],
        ),
      /frame-src lacks https:\/\/a\.example/,
    );
  });

  test('stages for Vercel without Cloudflare files or source maps', () => {
    const dist = fakeDist(join(tmp, 'vercel-dist'));
    const out = join(tmp, 'vercel-out');
    stage({ dist, out, env: AUTH, target: 'vercel', vercelJson: vercel });
    assert.ok(existsSync(join(out, 'index.html')));
    assert.ok(!existsSync(join(out, '_headers')));
    assert.ok(!existsSync(join(out, 'ui', 'assets', 'index-abc.js.map')));
    assert.ok(existsSync(join(out, 'ui', 'assets', 'index-abc.js')));
  });
});

describe('runtime config', () => {
  test('keeps an http(s) server origin and drops everything else', () => {
    assert.deepEqual(parseRuntimeConfig({ server: 'https://loams.example.com/x?y' }), {
      server: 'https://loams.example.com',
    });
    assert.deepEqual(parseRuntimeConfig({}), {});
    assert.deepEqual(parseRuntimeConfig({ server: '' }), {});
    assert.deepEqual(parseRuntimeConfig({ server: 'javascript:alert(1)' }), {});
    assert.deepEqual(parseRuntimeConfig({ server: 'not a url' }), {});
    assert.deepEqual(parseRuntimeConfig([]), {});
    assert.deepEqual(parseRuntimeConfig(null), {});
  });

  test('reads the hosted keys: engineUrl, the OIDC client, and the hosted marker', () => {
    assert.deepEqual(
      parseRuntimeConfig({
        hosted: true,
        engineUrl: 'https://loams.tail1.ts.net/',
        authIssuer: 'https://auth.loams.dev/',
        authClientId: ' loams-console ',
      }),
      {
        hosted: true,
        server: 'https://loams.tail1.ts.net',
        authMode: 'tailscale',
        auth: { issuer: 'https://auth.loams.dev', clientId: 'loams-console', scope: DEFAULT_SCOPE },
      },
    );
    // Hosted without a mode is tailscale, the default.
    assert.deepEqual(parseRuntimeConfig({ hosted: true }), {
      hosted: true,
      authMode: 'tailscale',
    });
    // An issuer without a client id (or over plain http off loopback) is no sign-in.
    assert.deepEqual(parseRuntimeConfig({ authIssuer: 'https://auth.loams.dev' }), {});
    assert.deepEqual(
      parseRuntimeConfig({ authIssuer: 'http://auth.example', authClientId: 'c' }),
      {},
    );
    // The mode is the default; the client stays in every mode, so oidc can be picked.
    const ts = parseRuntimeConfig({
      authMode: 'Tailscale',
      authIssuer: 'https://auth.loams.dev',
      authClientId: 'c',
    });
    assert.equal(ts.authMode, 'tailscale');
    assert.equal(ts.auth?.clientId, 'c');
    assert.deepEqual(offeredModes(ts), ['tailscale', 'oidc', 'apikey']);
    assert.deepEqual(parseRuntimeConfig({ authMode: 'apikey' }), {
      hosted: true,
      authMode: 'apikey',
    });
    assert.deepEqual(offeredModes(parseRuntimeConfig({ authMode: 'apikey' })), [
      'tailscale',
      'apikey',
    ]);
    // authModes narrows the offer; with one mode the connect screen has no picker.
    const onlyTailscale = parseRuntimeConfig({
      authMode: 'tailscale',
      authModes: ['tailscale', 'nonsense'],
    });
    assert.deepEqual(onlyTailscale.authModes, ['tailscale']);
    assert.deepEqual(offeredModes(onlyTailscale), ['tailscale']);
    // A default the deploy left out lands on a mode it did offer, never on that one.
    const narrowed = parseRuntimeConfig({ authMode: 'apikey', authModes: ['tailscale'] });
    assert.equal(narrowed.authMode, 'tailscale');
    assert.deepEqual(offeredModes(narrowed), ['tailscale']);
    // ... but the deploy's own mistake stays loud: oidc asked for without a client.
    assert.match(
      parseRuntimeConfig({ hosted: true, authMode: 'oidc', authModes: ['oidc'] }).configError,
      /authClientId/,
    );
    // A list that leaves nothing usable says so, rather than showing no way in.
    assert.match(parseRuntimeConfig({ authModes: ['oidc'] }).configError, /no sign-in mode/);
    // oidc as the default without a client is a config error, never a quiet fall back.
    assert.match(
      parseRuntimeConfig({ hosted: true, authMode: 'oidc' }).configError,
      /authClientId/,
    );
    // A hosted console sends a bearer: no plain-http engine off loopback. The
    // engine-served console keeps its LAN http engine.
    assert.equal(
      parseRuntimeConfig({ authMode: 'apikey', engineUrl: 'http://10.0.0.5:8080' }).server,
      undefined,
    );
    assert.equal(
      parseRuntimeConfig({ authMode: 'apikey', engineUrl: 'http://127.0.0.1:8080' }).server,
      'http://127.0.0.1:8080',
    );
    assert.equal(
      parseRuntimeConfig({ server: 'http://10.0.0.5:8080' }).server,
      'http://10.0.0.5:8080',
    );
    // An unknown mode is a config error, and keeps no OIDC client.
    for (const authMode of ['nope', 42, '']) {
      const bad = parseRuntimeConfig({
        authMode,
        authIssuer: 'https://a.example',
        authClientId: 'c',
      });
      assert.equal(bad.hosted, true);
      assert.equal(bad.authMode, undefined);
      assert.equal(bad.auth, undefined);
      assert.match(bad.configError, /authMode .* must be one of tailscale, oidc, apikey/);
    }
    assert.equal(
      parseRuntimeConfig({
        authIssuer: 'http://localhost:9000/application/o/loams/',
        authClientId: 'c',
        authScope: 'openid',
      }).auth?.issuer,
      'http://localhost:9000/application/o/loams',
    );
  });

  test('a stalled configuration request aborts and keeps the same-origin fallback', async () => {
    let aborted = false;
    const stalled = (_url, options) =>
      new Promise((_resolve, reject) => {
        options?.signal?.addEventListener('abort', () => {
          aborted = true;
          reject(new Error('aborted'));
        });
      });
    assert.deepEqual(await loadRuntimeConfig('/ui/', stalled, 10), {});
    assert.equal(aborted, true);
  });

  test('loads <base>config.json and ignores an HTML fallback or a failure', async () => {
    const seen = [];
    const json = async (url) => {
      seen.push(url);
      return Response.json({ server: 'https://a.example' });
    };
    assert.deepEqual(await loadRuntimeConfig('/ui/', json), { server: 'https://a.example' });
    assert.deepEqual(seen, ['/ui/config.json']);
    const html = async () =>
      new Response('<!doctype html>', { headers: { 'content-type': 'text/html' } });
    assert.deepEqual(await loadRuntimeConfig('/ui/', html), {});
    const down = async () => {
      throw new TypeError('offline');
    };
    assert.deepEqual(await loadRuntimeConfig('/ui/', down), {});
  });
});

describe('smoke', () => {
  test('finds the scripts and stylesheets a page loads', () => {
    assert.deepEqual(assetUrls(INDEX, 'https://console.loams.dev/ui/'), [
      'https://console.loams.dev/ui/assets/index-abc.js',
      'https://console.loams.dev/ui/assets/index-abc.css',
    ]);
    assert.ok(isJavaScript('text/javascript; charset=utf-8'));
    assert.ok(isJavaScript('application/javascript'));
    assert.ok(!isJavaScript('text/html'));
  });

  /** A fake deploy: `fallbackAssets` reproduces the 2026-10-02 bug (assets served as the HTML fallback). */
  const site = (fallbackAssets) => async (url) => {
    const { pathname } = new URL(url);
    const html = () => {
      const res = new Response(INDEX, { headers: { 'content-type': 'text/html; charset=utf-8' } });
      Object.defineProperty(res, 'url', { value: 'https://console.loams.dev/ui/' });
      return res;
    };
    if (pathname.endsWith('.js') && !fallbackAssets)
      return new Response('export {}', { headers: { 'content-type': 'text/javascript' } });
    if (pathname.endsWith('.css') && !fallbackAssets)
      return new Response('', { headers: { 'content-type': 'text/css' } });
    if (pathname === '/config.json') return Response.json({ hosted: true });
    return html();
  };

  test('passes a good deploy', async () => {
    assert.deepEqual(await smoke('https://console.loams.dev', site(false)), []);
  });

  test('fails a hosted deploy with missing, invalid or mismatched API configuration', async () => {
    for (const value of [
      { hosted: true },
      { hosted: true, engineUrl: 'not a url' },
      { hosted: true, engineUrl: 'https://wrong.example' },
    ]) {
      const fetcher = async (url) =>
        new URL(url).pathname === '/config.json' ? Response.json(value) : site(false)(url);
      const failures = await smoke('https://console.loams.dev', fetcher, 'https://api.example');
      assert.ok(
        failures.some((failure) => failure.includes('server origin')),
        failures.join('\n'),
      );
    }
  });

  test('compares normalized API origins after a hosted deploy', async () => {
    const fetcher = async (url) =>
      new URL(url).pathname === '/config.json'
        ? Response.json({ hosted: true, engineUrl: 'https://api.example/path' })
        : site(false)(url);
    assert.deepEqual(
      await smoke('https://console.loams.dev', fetcher, 'https://api.example/other'),
      [],
    );
  });

  test('fails a deploy whose config is not the hosted console, or names another issuer', async () => {
    const engineConfig = async (url) =>
      new URL(url).pathname === '/config.json' ? Response.json({}) : site(false)(url);
    const failures = await smoke('https://console.loams.dev', engineConfig);
    assert.ok(
      failures.some((f) => f.includes('hosted')),
      failures.join('\n'),
    );
    const issuer = await smoke(
      'https://console.loams.dev',
      site(false),
      undefined,
      'https://auth.loams.dev',
    );
    assert.ok(
      issuer.some((f) => f.includes('authIssuer')),
      issuer.join('\n'),
    );
    // Another realm on the same host is another issuer; a trailing slash is not.
    const realm = (authIssuer) => async (url) =>
      new URL(url).pathname === '/config.json'
        ? Response.json({ hosted: true, authIssuer })
        : site(false)(url);
    const want = 'https://id.example/realms/loams';
    const other = await smoke(
      'https://console.loams.dev',
      realm('https://id.example/realms/x'),
      undefined,
      want,
    );
    assert.ok(
      other.some((f) => f.includes('authIssuer')),
      other.join('\n'),
    );
    assert.deepEqual(
      await smoke('https://console.loams.dev', realm(`${want}/`), undefined, want),
      [],
    );
  });

  test('fails a deploy whose config has another sign-in mode', async () => {
    const mode = (authMode) => async (url) =>
      new URL(url).pathname === '/config.json'
        ? Response.json({ hosted: true, authMode })
        : site(false)(url);
    const wrong = await smoke(
      'https://console.loams.dev',
      mode('apikey'),
      undefined,
      undefined,
      'oidc',
    );
    assert.ok(
      wrong.some((f) => f.includes('authMode is apikey, expected oidc')),
      wrong.join('\n'),
    );
    assert.deepEqual(
      await smoke('https://console.loams.dev', mode('oidc'), undefined, undefined, 'OIDC'),
      [],
    );
  });

  test('fails when scripts come back as the HTML fallback', async () => {
    const failures = await smoke('https://console.loams.dev', site(true));
    assert.ok(
      failures.some((f) => /index-abc\.js is text\/html/.test(f)),
      failures.join('\n'),
    );
  });
});
