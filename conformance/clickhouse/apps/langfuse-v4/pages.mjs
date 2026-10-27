// HE0 (plan HE1 Tasks 3 and 9): sign in to Langfuse as the driver's user and
// open its main pages in a headless browser, so the UI's own tRPC calls send
// the dashboard, table and detail queries to ClickHouse. A page passes when
// every tRPC and public API call it makes answers below 400 (a 404 for a
// detail page whose entity is not stored yet is a failure too: the driver
// opens only entities it ingested). Exit status 1 on any failure.
//
//   HE0_LANGFUSE_URL   default http://127.0.0.1:${HE0_LANGFUSE_PORT:-3100}
//   HE0_PAGES          comma-separated paths under /project/<id>/ ("/" is the
//                      project home); default: every main page
//   HE0_DETAIL         "<trace id>,<session id>,<user id>": also open those
//                      detail pages
//
// Synthetic traffic only: the user, project and keys are compose.yaml's.
import { chromium } from "playwright";

const URL =
  process.env.HE0_LANGFUSE_URL ?? `http://127.0.0.1:${process.env.HE0_LANGFUSE_PORT ?? 3100}`;
const PROJECT = "he0-project";
const MAIN = [
  "/",
  "traces",
  "observations",
  "sessions",
  "users",
  "scores",
  "dashboards",
  "datasets",
  "prompts",
  "evals",
  "annotation-queues",
  "settings",
];
const pages = process.env.HE0_PAGES ? process.env.HE0_PAGES.split(",") : [...MAIN];
if (process.env.HE0_DETAIL) {
  const [trace, session, user] = process.env.HE0_DETAIL.split(",");
  if (trace) pages.push(`traces/${trace}`, `traces?peek=${trace}`);
  if (session) pages.push(`sessions/${encodeURIComponent(session)}`);
  if (user) pages.push(`users/${encodeURIComponent(user)}`);
}

const browser = await chromium.launch();
const page = await (await browser.newContext()).newPage();
const failures = [];
let calls = 0;
page.on("response", async (response) => {
  const url = response.url();
  if (!url.includes("/api/trpc/") && !url.includes("/api/public/")) return;
  calls++;
  if (response.status() < 400) return;
  const body = (await response.text().catch(() => "")).slice(0, 600);
  failures.push({ page: current, status: response.status(), call: url.split("?")[0].replace(URL, ""), body });
});

let current = "/auth/sign-in";
await page.goto(`${URL}/auth/sign-in`);
await page.fill('input[name="email"]', "he0@example.com");
await page.fill('input[type="password"]', "he0-password");
await page.click('button[type="submit"]');
await page.waitForURL((u) => !u.pathname.startsWith("/auth"), { timeout: 60_000 });

for (const p of pages) {
  current = p === "/" ? `/project/${PROJECT}` : `/project/${PROJECT}/${p}`;
  const before = failures.length;
  let status;
  try {
    const response = await page.goto(`${URL}${current}`, { waitUntil: "networkidle", timeout: 120_000 });
    status = response?.status();
  } catch (err) {
    status = String(err).split("\n")[0];
    failures.push({ page: current, status, call: "(navigation)", body: "" });
  }
  // Charts and tables fetch after first paint.
  await page.waitForTimeout(1_500);
  console.log(`pages: ${current}: ${status}, ${failures.length - before} failed calls`);
}
await browser.close();
console.log(`pages: ${pages.length} pages, ${calls} calls, ${failures.length} failed`);
for (const f of failures) console.log(`pages: FAIL ${JSON.stringify(f)}`);
process.exit(failures.length ? 1 : 0);
