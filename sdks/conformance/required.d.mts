// Types for `required.mjs`, so a TypeScript SDK's suite can write the report
// the rule reads (`sdks/conformance/required.mjs`, design §44 §10.4, D617).
//
// The rule lives in that one file and is the authority; this declaration is the
// window onto **three** of its exports and says nothing the module does not.
// It is deliberately short: every export added here is a second place to keep
// in step, so a suite that needs more reads the corpus directly instead.
//
// The runtime is Node's own loader, so nothing here affects what executes —
// this only stops `tsc` from reporting the module as an untyped `any`, which is
// the difference between a report that is checked and a report nobody notices
// is missing.

/** A fixture a language's suite must run: see `REQUIRED_TESTS`. */
export interface ConformanceReport {
  /** The six canonical test names, `<language>_<short>`. */
  tests?: string[];
  /**
   * The fixtures the suite **actually exercised**. The only field the 100% bar
   * reads: a test that passes without touching a required fixture has not run
   * it, so nothing else may be added here as cover.
   */
  ran?: string[];
  /**
   * Fixtures not run, with why. Legal only for a fixture marked
   * `transport: grpc-only` on the Connect-unary fallback (D613); every other
   * entry is a failure whatever its reason text says.
   */
  skipped?: { fixture: string; reason: string }[];
  /** The transport the run used, which `maySkip` keys off. */
  transport?: string;
}

/**
 * The six tests every language's suite has, in the plan's order, as short
 * names. `testName(language, short)` is the full name.
 */
export const REQUIRED_TESTS: string[];

/** The full name of one of a language's six tests: `` `${language}_${short}` ``. */
export function testName(language: string, short: string): string;

/**
 * Writes `sdks/fixtures/results/<language>.json`, creating `results/` if it is
 * not there. Used by a suite, and by the runner's own checks.
 *
 * @param language the suite's language, which names the file.
 * @param report what it ran; see {@link ConformanceReport}.
 * @param dir the fixture root; the module's own by default.
 */
export function saveReport(
  language: string,
  report: ConformanceReport,
  dir?: string,
): Promise<void>;