# VV0 report: Void and Vite+ spikes

> **Status: in progress.** Track **VV**, plan [VV1](2026-10-23-vv1-void-and-vite-plus.md), design [§79](../design/79-void-and-vite-plus.md).
> Each task's PR fills its own section below. Spike code lives under `spikes/vv0/<task>/` in the task's worktree and is not committed; each section gives the commands to reproduce it.
>
> **Accounts.** Tasks 2, 3, 5 and 7 deploy only to the owner's Cloudflare Free test account and AWS credit account (§70 §12.2), with scoped tokens the owner provides. Until the owner provides them, those tasks report only what can be proven locally, and label the rest "blocked".

## Summary

| Task | Question | Verdict |
|---|---|---|
| 1 | The console and `@loams/ui` under Vite+ | **Go.** Tests, `dist/` and `@loams/ui`'s exports are unchanged. VV1 must switch the scripts to `vp`, set `fixedExtension: false` and `tabWidth: 2`, and rewrite 107 `biome-ignore` comments. Lint parity: 169 of 223 rules mapped, 5 covered by tsc, 14 lost, 25 CSS rules lost |
| 2 | `@loams/app` ports through Void's Cloudflare bindings | **Go locally** (Miniflare) for the D1, R2 and Queues bindings through a generated overlay. A DLQ can be bound explicitly, but Void appends a second consumer, so the overlay must replace the inferred one. Provisioning, the lock file and rollback are blocked on the owner's account |
| 3 | Loams' shims around Void's `node` build | **Go locally** for HTTP, cron and queue through one Lambda handler (unit tests and the Lambda emulator). `node` exports only the Hono app, so crons and queues are called through the build manifest. Real Lambda cold starts are blocked on the owner's AWS account |
| 4 | Bun-compiled and `node` images in the light compose stack | **Go**: both images serve, hydrate and persist (Postgres row and bucket body) across app and stack restarts, through `env.native`. Ran in a spike compose with an `apps` profile, because the release compose has none yet. Postgres 17 stood in for Loams Postgres |
| 5 | Hyperdrive to Loams Postgres over Tunnel, and TiDB Starter | pending |
| 6 | One `vite-plus` pin for the workspace | **Go.** 171 build, test and typecheck tasks in 75 projects pass on one `vite-plus` 1.3.0 pin. The peer warnings clear only with `peerDependencyRules.allowedVersions.vite`. electron-vite 6.0.0 runs on `vite-plus-core`. By the 14-day rule, the first pin is 1.1.0, with the desktop following on 2026-10-28 |
| 7 | The Loams Cloud control plane on Void | pending |
| 8 | Bun's ahead-of-time compile against Node | **Go** on cold start (5.5x faster than Node 24 for the process, 1.5x for the container); the image is 138 MB on distroless glibc and 118 MB on Alpine musl, so the under-120 MB target is met only on musl. Embedding `dist/client/` needs a one-line rewrite of Void's client root |

## Task 1

**Question.** Does the console build, test and lint under Vite+ with no behaviour change?

**Verdict: go**, with four changes that VV1 must make on purpose (listed under "What VV1 must do"). Tests, `dist/` and the `@loams/ui` package surface are unchanged. Lint and format are not a drop-in swap: the parity table below names every gap.

**Set-up (2026-10-24).** `vite-plus` 1.3.0, which bundles Vite 8.3.4, Rolldown 1.2.14, Vitest 5.0.5, tsdown 0.23.1, Oxlint 1.88.0 and Oxfmt 0.73.0. The baseline is `dev` at `710f470`: Vite 8.3.1, Vitest 5.0.3, tsup 8.5.1 and Biome 2.5.15. Node 26.8.2 and pnpm 11.27.1 on the shared 14-core build machine. Every timed command ran inside a `buildslot.sh` slot, while other agents' cargo builds held the other slots (load average 7 to 11). So the wall times are noisy. Read them as "no regression", not as a speed-up claim.

### Results

| Step | Baseline | Vite+ | Same result? |
|---|---|---|---|
| Console build | `vite build`, median 4.4 s of 5 (Vite's own figure 1.68 s) | `vp build`, median 2.6 s of 5 (1.48 s) | Yes: the same 982 files; JS −3.9 KB (−0.02%), CSS and HTML byte-identical |
| Console tests | `vitest run`, median 7.1 s of 3 | `vp test run`, median 4.3 s of 3 | Yes: 17 files and 150 tests passed in both |
| `@loams/ui` build | `tsup`, median 3.6 s of 3 | `vp pack`, median 2.4 s of 3 | Yes, with `fixedExtension: false`: the same 281 runtime exports, the same 331 declaration exports, and the same external imports. JS is 4.7% smaller (229.9 KB against 241.2 KB) |
| `@loams/ui` tests | `vitest run`, median 3.4 s of 3 | `vp test run`, median 3.5 s of 3 | Yes: 15 files and 266 tests passed in both |
| `web/` lint and format | `biome check`, median 0.73 s of 3 | `vp check`, median 0.78 s of 3 (`vp lint` 0.42 s, `vp fmt --check` 0.60 s) | No. See the parity table and "Lint" below |

`dist/` was compared file by file after removing the content hashes from the names: 873 names on both sides, and 392 files are byte-identical even with the hashes left inside them. The rest differ only in hashed chunk references and source-map offsets. Totals per type are the same within 0.02%.

### What VV1 must do

1. **Scripts call `vp`, not `vite`.** With `vite` aliased to `@voidzero-dev/vite-plus-core`, `node_modules/.bin/vite` no longer exists. The console's `dev`, `build` and `preview` scripts (`vite`, `vite build`, `vite preview`) fail until they become `vp dev`, `vp build` and `vp preview`. `vitest` still has a binary, since Vite+ 1.3.0 depends on the real Vitest 5.0.5.
2. **`vp pack` needs `fixedExtension: false` in `@loams/ui`.** tsdown 0.23 writes `index.mjs` and `index.d.mts` by default. `@loams/ui`'s `publishConfig.exports` names `dist/index.js` and `dist/index.d.ts`, so the published package would break silently. With the option set, the output matches. tsup's `external: ['react', 'react/jsx-runtime']` needs no counterpart: tsdown keeps peer dependencies, and their subpaths, external.
3. **Oxfmt reads `.editorconfig`.** The root `.editorconfig` says `indent_size = 4`, and Biome ignored it because `biome.json` sets 2. Without an explicit `tabWidth: 2`, Oxfmt would reindent 874 of 995 files. With `singleQuote`, `printWidth: 100`, `tabWidth: 2` and Biome's two double-quote overrides, 72 files change:
   - 10 TypeScript files (260 lines), mostly where Oxfmt keeps a typed arrow callback's argument on one line;
   - 62 files Biome didn't format: 46 JSON, 1 JSONC, 6 Markdown, 6 CSS, 2 HTML and 1 `.mjs`.

   That is one formatting commit, listed in `.git-blame-ignore-revs` (D1773).
4. **107 `biome-ignore` comments need rewriting** as `oxlint-disable-next-line` comments, because Oxlint doesn't read them. The 77 lint suppressions are: `useExhaustiveDependencies` 48, `noArrayIndexKey` 21, `noControlCharactersInRegex` 3, and one each of `noTemplateCurlyInString`, `noDescendingSpecificity`, `noDangerouslySetInnerHtml`, `useYield` and `useHookAtTopLevel`. Without the rewrite, those warnings come back.

### Lint

Biome's effective rule set for `web/` is its `recommended` preset plus the React domain's recommended rules: 223 rules. The table maps each one to an Oxlint 1.88.0 rule, or names the gap.

| Class | Rules | Meaning |
|---|---|---|
| Mapped | 169 | An Oxlint rule with the same intent: 67 matched by name, and 102 mapped by hand (each checked to exist in Oxlint 1.88.0; a reviewer should spot-check intent) |
| Covered by tsc or strict mode | 5 | `noDuplicateParameters`, `noOctalEscape`, `noVoidTypeReturn`, `noImplicitAnyLet` and `noRedundantUseStrict`. `tsc` with `strict`, or ES module parsing, already rejects these, and every package type-checks |
| Lost: no Oxlint rule | 14 | `noApproximativeNumericConstant`, `noConstantMathMinMaxClamp`, `noDynamicNamespaceImportAccess`, `noEmptyTypeParameters`, `noFlatMapIdentity`, `noMisrefactoredShorthandAssign`, `noStringCaseMismatch`, `noSuspiciousSemicolonInJsx`, `noSvgWithoutTitle`, `noThisInStatic`, `noUselessContinue`, `noUselessStringRaw`, `noUselessUndefinedInitialization` and `useSimpleNumberKeys`. None fires on `web/` today |
| Lost: CSS | 25 | Oxlint lints only JavaScript and TypeScript. Biome's CSS rules go, including `noDescendingSpecificity`, which has 6 warnings in `console.css` today. Oxfmt still formats CSS. Getting CSS lint back means adding Stylelint, which this report doesn't recommend |
| Not applicable | 10 | 7 GraphQL rules (no GraphQL in `web/`), and 3 Biome-only rules about Biome's own suppression comments |

The same code, with the mapped rules all set to `warn`:
- Biome reports **68 warnings and 6 infos**.
- Oxlint reports **697 warnings**.

The difference is semantic, not missing rules:
- **`no-use-before-define`, 331 warnings.** Biome's `noInvalidUseBeforeDeclaration` flags only temporal-dead-zone errors. ESLint's rule also flags hoisted functions. VV1 sets it to `{ functions: false, classes: false, variables: false, typedefs: false, ignoreTypeReferences: true }`.
- **`jsx-a11y/prefer-tag-over-role`, 89 warnings.** Biome's `useSemanticElements` is narrower. Each one is a real a11y suggestion (for example, `role="status"` should be `<output>`). Keep it as a warning and fix it over time.
- **`no-non-null-assertion`, 110 warnings against Biome's 44.** The spike config left out `biome.json`'s per-file overrides. VV1 carries them over into `lint.overrides`.
- **`exhaustive-deps` and `no-array-index-key`, 49 warnings.** These are the `biome-ignore` comments from item 4 above.

### Reproduce

Run these in a worktree off `dev`:
1. Apply Task 6's catalog pin and override.
2. Switch the console's and `@loams/ui`'s scripts to `vp`.
3. Add `web/packages/ui/vite.config.ts` with `pack: { entry: ['src/index.ts'], format: ['esm'], dts: true, clean: true, fixedExtension: false }`.
4. Add `web/vite.config.ts` with the `lint` rules from the mapping, and `fmt: { singleQuote: true, printWidth: 100, useTabs: false, tabWidth: 2 }`.
5. Run `vp build` and `vp test run` in `web/apps/console`, `vp pack` and `vp test run` in `web/packages/ui`, and `vp check` in `web/`.

The rule mapping is generated from two lists:
- `biome explain <rule>` for every rule in Biome's configuration schema, which gives "recommended" and the recommended domains;
- `oxlint --rules --format=json`.

<details><summary>The full parity table (223 rules)</summary>

| Biome rule | Oxlint rule | Class |
|---|---|---|
| `a11y/noAccessKey` | `jsx_a11y/no-access-key` | mapped (by name) |
| `a11y/noAmbiguousAnchorText` | `jsx_a11y/anchor-ambiguous-text` | mapped (by hand) |
| `a11y/noAriaHiddenOnFocusable` | `jsx_a11y/no-aria-hidden-on-focusable` | mapped (by name) |
| `a11y/noAriaUnsupportedElements` | `jsx_a11y/aria-unsupported-elements` | mapped (by hand) |
| `a11y/noAutofocus` | `jsx_a11y/no-autofocus` | mapped (by name) |
| `a11y/noDistractingElements` | `jsx_a11y/no-distracting-elements` | mapped (by name) |
| `a11y/noHeaderScope` | `jsx_a11y/scope` | mapped (by hand) |
| `a11y/noInteractiveElementToNoninteractiveRole` | `jsx_a11y/no-interactive-element-to-noninteractive-role` | mapped (by name) |
| `a11y/noLabelWithoutControl` | `jsx_a11y/label-has-associated-control` | mapped (by hand) |
| `a11y/noNoninteractiveElementToInteractiveRole` | `jsx_a11y/no-noninteractive-element-to-interactive-role` | mapped (by name) |
| `a11y/noNoninteractiveTabindex` | `jsx_a11y/no-noninteractive-tabindex` | mapped (by name) |
| `a11y/noPositiveTabindex` | `jsx_a11y/tabindex-no-positive` | mapped (by hand) |
| `a11y/noRedundantAlt` | `jsx_a11y/img-redundant-alt` | mapped (by hand) |
| `a11y/noRedundantRoles` | `jsx_a11y/no-redundant-roles` | mapped (by name) |
| `a11y/noStaticElementInteractions` | `jsx_a11y/no-static-element-interactions` | mapped (by name) |
| `a11y/useAltText` | `jsx_a11y/alt-text` | mapped (by name) |
| `a11y/useAnchorContent` | `jsx_a11y/anchor-has-content` | mapped (by hand) |
| `a11y/useAriaActivedescendantWithTabindex` | `jsx_a11y/aria-activedescendant-has-tabindex` | mapped (by hand) |
| `a11y/useAriaPropsForRole` | `jsx_a11y/role-has-required-aria-props` | mapped (by hand) |
| `a11y/useAriaPropsSupportedByRole` | `jsx_a11y/role-supports-aria-props` | mapped (by hand) |
| `a11y/useButtonType` | `react/button-has-type` | mapped (by hand) |
| `a11y/useFocusableInteractive` | `jsx_a11y/interactive-supports-focus` | mapped (by hand) |
| `a11y/useHeadingContent` | `jsx_a11y/heading-has-content` | mapped (by hand) |
| `a11y/useHtmlLang` | `jsx_a11y/html-has-lang` | mapped (by hand) |
| `a11y/useIframeTitle` | `jsx_a11y/iframe-has-title` | mapped (by hand) |
| `a11y/useKeyWithClickEvents` | `jsx_a11y/click-events-have-key-events` | mapped (by hand) |
| `a11y/useKeyWithMouseEvents` | `jsx_a11y/mouse-events-have-key-events` | mapped (by hand) |
| `a11y/useMediaCaption` | `jsx_a11y/media-has-caption` | mapped (by hand) |
| `a11y/useSemanticElements` | `jsx_a11y/prefer-tag-over-role` | mapped (by hand) |
| `a11y/useValidAnchor` | `jsx_a11y/anchor-is-valid` | mapped (by hand) |
| `a11y/useValidAriaProps` | `jsx_a11y/aria-props` | mapped (by hand) |
| `a11y/useValidAriaRole` | `jsx_a11y/aria-role` | mapped (by hand) |
| `a11y/useValidAriaValues` | `jsx_a11y/aria-proptypes` | mapped (by hand) |
| `a11y/useValidAutocomplete` | `jsx_a11y/autocomplete-valid` | mapped (by hand) |
| `a11y/useValidLang` | `jsx_a11y/lang` | mapped (by hand) |
| `complexity/noAdjacentSpacesInRegex` | `eslint/no-regex-spaces` | mapped (by hand) |
| `complexity/noArguments` | `eslint/prefer-rest-params` | mapped (by hand) |
| `complexity/noBannedTypes` | `typescript/no-unsafe-function-type` | mapped (by hand) |
| `complexity/noCommaOperator` | `eslint/no-sequences` | mapped (by hand) |
| `complexity/noExtraBooleanCast` | `eslint/no-extra-boolean-cast` | mapped (by name) |
| `complexity/noStaticOnlyClass` | `unicorn/no-static-only-class` | mapped (by name) |
| `complexity/noUselessCatch` | `eslint/no-useless-catch` | mapped (by name) |
| `complexity/noUselessConstructor` | `eslint/no-useless-constructor` | mapped (by name) |
| `complexity/noUselessEmptyExport` | `typescript/no-useless-empty-export` | mapped (by name) |
| `complexity/noUselessEscapeInRegex` | `eslint/no-useless-escape` | mapped (by hand) |
| `complexity/noUselessFragments` | `react/jsx-no-useless-fragment` | mapped (by hand) |
| `complexity/noUselessLabel` | `eslint/no-extra-label` | mapped (by hand) |
| `complexity/noUselessLoneBlockStatements` | `eslint/no-lone-blocks` | mapped (by hand) |
| `complexity/noUselessRename` | `eslint/no-useless-rename` | mapped (by name) |
| `complexity/noUselessSwitchCase` | `unicorn/no-useless-switch-case` | mapped (by name) |
| `complexity/noUselessTernary` | `eslint/no-unneeded-ternary` | mapped (by hand) |
| `complexity/noUselessThisAlias` | `typescript/no-this-alias` | mapped (by hand) |
| `complexity/noUselessTypeConstraint` | `typescript/no-unnecessary-type-constraint` | mapped (by hand) |
| `complexity/useArrowFunction` | `eslint/prefer-arrow-callback` | mapped (by hand) |
| `complexity/useDateNow` | `unicorn/prefer-date-now` | mapped (by hand) |
| `complexity/useFlatMap` | `unicorn/prefer-array-flat-map` | mapped (by hand) |
| `complexity/useIndexOf` | `unicorn/prefer-array-index-of` | mapped (by hand) |
| `complexity/useLiteralKeys` | `typescript/dot-notation` | mapped (by hand) |
| `complexity/useNumericLiterals` | `eslint/prefer-numeric-literals` | mapped (by hand) |
| `complexity/useOptionalChain` | `typescript/prefer-optional-chain` | mapped (by hand) |
| `complexity/useRegexLiterals` | `eslint/prefer-regex-literals` | mapped (by hand) |
| `correctness/noChildrenProp` | `react/no-children-prop` | mapped (by name) |
| `correctness/noConstAssign` | `eslint/no-const-assign` | mapped (by name) |
| `correctness/noConstantCondition` | `eslint/no-constant-condition` | mapped (by name) |
| `correctness/noConstructorReturn` | `eslint/no-constructor-return` | mapped (by name) |
| `correctness/noDuplicateEnumValueNames` | `typescript/no-duplicate-enum-values` | mapped (by hand) |
| `correctness/noEmptyCharacterClassInRegex` | `eslint/no-empty-character-class` | mapped (by hand) |
| `correctness/noEmptyPattern` | `eslint/no-empty-pattern` | mapped (by name) |
| `correctness/noGlobalObjectCalls` | `eslint/no-obj-calls` | mapped (by hand) |
| `correctness/noInnerDeclarations` | `eslint/no-inner-declarations` | mapped (by name) |
| `correctness/noInvalidBuiltinInstantiation` | `eslint/no-new-native-nonconstructor` | mapped (by hand) |
| `correctness/noInvalidConstructorSuper` | `eslint/constructor-super` | mapped (by hand) |
| `correctness/noInvalidUseBeforeDeclaration` | `eslint/no-use-before-define` | mapped (by hand) |
| `correctness/noNonoctalDecimalEscape` | `eslint/no-nonoctal-decimal-escape` | mapped (by name) |
| `correctness/noPrecisionLoss` | `eslint/no-loss-of-precision` | mapped (by hand) |
| `correctness/noRenderReturnValue` | `react/no-render-return-value` | mapped (by name) |
| `correctness/noSelfAssign` | `eslint/no-self-assign` | mapped (by name) |
| `correctness/noSetterReturn` | `eslint/no-setter-return` | mapped (by name) |
| `correctness/noSwitchDeclarations` | `eslint/no-case-declarations` | mapped (by hand) |
| `correctness/noUnknownProperty` | `react/no-unknown-property` | mapped (by name) |
| `correctness/noUnreachable` | `eslint/no-unreachable` | mapped (by name) |
| `correctness/noUnreachableSuper` | `eslint/no-this-before-super` | mapped (by hand) |
| `correctness/noUnsafeFinally` | `eslint/no-unsafe-finally` | mapped (by name) |
| `correctness/noUnsafeOptionalChaining` | `eslint/no-unsafe-optional-chaining` | mapped (by name) |
| `correctness/noUnusedFunctionParameters` | `eslint/no-unused-vars` | mapped (by hand) |
| `correctness/noUnusedImports` | `eslint/no-unused-vars` | mapped (by hand) |
| `correctness/noUnusedLabels` | `eslint/no-unused-labels` | mapped (by name) |
| `correctness/noUnusedPrivateClassMembers` | `eslint/no-unused-private-class-members` | mapped (by name) |
| `correctness/noUnusedVariables` | `eslint/no-unused-vars` | mapped (by hand) |
| `correctness/noVoidElementsWithChildren` | `react/void-dom-elements-no-children` | mapped (by hand) |
| `correctness/useExhaustiveDependencies` | `react/exhaustive-deps` | mapped (by hand) |
| `correctness/useHookAtTopLevel` | `react/rules-of-hooks` | mapped (by hand) |
| `correctness/useIsNan` | `eslint/use-isnan` | mapped (by hand) |
| `correctness/useJsxKeyInIterable` | `react/jsx-key` | mapped (by hand) |
| `correctness/useParseIntRadix` | `eslint/radix` | mapped (by hand) |
| `correctness/useValidForDirection` | `eslint/for-direction` | mapped (by hand) |
| `correctness/useValidTypeof` | `eslint/valid-typeof` | mapped (by name) |
| `correctness/useYield` | `eslint/require-yield` | mapped (by hand) |
| `performance/noAccumulatingSpread` | `oxc/no-accumulating-spread` | mapped (by name) |
| `security/noBlankTarget` | `react/jsx-no-target-blank` | mapped (by hand) |
| `security/noDangerouslySetInnerHtml` | `react/no-danger` | mapped (by hand) |
| `security/noDangerouslySetInnerHtmlWithChildren` | `react/no-danger-with-children` | mapped (by hand) |
| `security/noGlobalEval` | `eslint/no-eval` | mapped (by hand) |
| `security/noScriptUrl` | `eslint/no-script-url` | mapped (by name) |
| `style/noNonNullAssertion` | `typescript/no-non-null-assertion` | mapped (by name) |
| `style/useArrayLiterals` | `eslint/no-array-constructor` | mapped (by hand) |
| `style/useConst` | `eslint/prefer-const` | mapped (by hand) |
| `style/useExponentiationOperator` | `eslint/prefer-exponentiation-operator` | mapped (by hand) |
| `style/useExportType` | `typescript/consistent-type-exports` | mapped (by hand) |
| `style/useImportType` | `typescript/consistent-type-imports` | mapped (by hand) |
| `style/useLiteralEnumMembers` | `typescript/prefer-literal-enum-member` | mapped (by hand) |
| `style/useNodejsImportProtocol` | `unicorn/prefer-node-protocol` | mapped (by hand) |
| `style/useShorthandFunctionType` | `typescript/prefer-function-type` | mapped (by hand) |
| `style/useTemplate` | `eslint/prefer-template` | mapped (by hand) |
| `suspicious/noArrayIndexKey` | `react/no-array-index-key` | mapped (by name) |
| `suspicious/noAssignInExpressions` | `eslint/no-cond-assign` | mapped (by hand) |
| `suspicious/noAsyncPromiseExecutor` | `eslint/no-async-promise-executor` | mapped (by name) |
| `suspicious/noCatchAssign` | `eslint/no-ex-assign` | mapped (by hand) |
| `suspicious/noClassAssign` | `eslint/no-class-assign` | mapped (by name) |
| `suspicious/noCommentText` | `react/jsx-no-comment-textnodes` | mapped (by hand) |
| `suspicious/noCompareNegZero` | `eslint/no-compare-neg-zero` | mapped (by name) |
| `suspicious/noConfusingLabels` | `eslint/no-labels` | mapped (by hand) |
| `suspicious/noConfusingVoidType` | `typescript/no-invalid-void-type` | mapped (by hand) |
| `suspicious/noConstEnum` | `oxc/no-const-enum` | mapped (by name) |
| `suspicious/noControlCharactersInRegex` | `eslint/no-control-regex` | mapped (by hand) |
| `suspicious/noDebugger` | `eslint/no-debugger` | mapped (by name) |
| `suspicious/noDocumentCookie` | `unicorn/no-document-cookie` | mapped (by name) |
| `suspicious/noDoubleEquals` | `eslint/eqeqeq` | mapped (by hand) |
| `suspicious/noDuplicateCase` | `eslint/no-duplicate-case` | mapped (by name) |
| `suspicious/noDuplicateClassMembers` | `eslint/no-dupe-class-members` | mapped (by hand) |
| `suspicious/noDuplicateElseIf` | `eslint/no-dupe-else-if` | mapped (by hand) |
| `suspicious/noDuplicateEnumValues` | `typescript/no-duplicate-enum-values` | mapped (by name) |
| `suspicious/noDuplicateJsxProps` | `react/jsx-no-duplicate-props` | mapped (by hand) |
| `suspicious/noDuplicateObjectKeys` | `eslint/no-dupe-keys` | mapped (by hand) |
| `suspicious/noEmptyInterface` | `typescript/no-empty-interface` | mapped (by name) |
| `suspicious/noExplicitAny` | `typescript/no-explicit-any` | mapped (by name) |
| `suspicious/noExtraNonNullAssertion` | `typescript/no-extra-non-null-assertion` | mapped (by name) |
| `suspicious/noFallthroughSwitchClause` | `eslint/no-fallthrough` | mapped (by hand) |
| `suspicious/noFunctionAssign` | `eslint/no-func-assign` | mapped (by hand) |
| `suspicious/noGlobalAssign` | `eslint/no-global-assign` | mapped (by name) |
| `suspicious/noGlobalIsFinite` | `unicorn/prefer-number-properties` | mapped (by hand) |
| `suspicious/noGlobalIsNan` | `unicorn/prefer-number-properties` | mapped (by hand) |
| `suspicious/noImportAssign` | `eslint/no-import-assign` | mapped (by name) |
| `suspicious/noIrregularWhitespace` | `eslint/no-irregular-whitespace` | mapped (by name) |
| `suspicious/noLabelVar` | `eslint/no-label-var` | mapped (by name) |
| `suspicious/noMisleadingCharacterClass` | `eslint/no-misleading-character-class` | mapped (by name) |
| `suspicious/noMisleadingInstantiator` | `typescript/no-misused-new` | mapped (by hand) |
| `suspicious/noNonNullAssertedOptionalChain` | `typescript/no-non-null-asserted-optional-chain` | mapped (by name) |
| `suspicious/noProto` | `eslint/no-proto` | mapped (by name) |
| `suspicious/noPrototypeBuiltins` | `eslint/no-prototype-builtins` | mapped (by name) |
| `suspicious/noRedeclare` | `eslint/no-redeclare` | mapped (by name) |
| `suspicious/noSelfCompare` | `eslint/no-self-compare` | mapped (by name) |
| `suspicious/noShadowRestrictedNames` | `eslint/no-shadow-restricted-names` | mapped (by name) |
| `suspicious/noSparseArray` | `eslint/no-sparse-arrays` | mapped (by hand) |
| `suspicious/noTemplateCurlyInString` | `eslint/no-template-curly-in-string` | mapped (by name) |
| `suspicious/noThenProperty` | `unicorn/no-thenable` | mapped (by hand) |
| `suspicious/noTsIgnore` | `typescript/ban-ts-comment` | mapped (by hand) |
| `suspicious/noUnsafeDeclarationMerging` | `typescript/no-unsafe-declaration-merging` | mapped (by name) |
| `suspicious/noUnsafeNegation` | `eslint/no-unsafe-negation` | mapped (by name) |
| `suspicious/noUselessEscapeInString` | `eslint/no-useless-escape` | mapped (by hand) |
| `suspicious/noUselessRegexBackrefs` | `eslint/no-useless-backreference` | mapped (by hand) |
| `suspicious/noWith` | `eslint/no-with` | mapped (by name) |
| `suspicious/useAdjacentOverloadSignatures` | `typescript/adjacent-overload-signatures` | mapped (by name) |
| `suspicious/useDefaultSwitchClauseLast` | `eslint/default-case-last` | mapped (by hand) |
| `suspicious/useGetterReturn` | `eslint/getter-return` | mapped (by name) |
| `suspicious/useGoogleFontDisplay` | `nextjs/google-font-display` | mapped (by name) |
| `suspicious/useIsArray` | `unicorn/no-instanceof-array` | mapped (by hand) |
| `suspicious/useIterableCallbackReturn` | `eslint/array-callback-return` | mapped (by hand) |
| `suspicious/useNamespaceKeyword` | `typescript/prefer-namespace-keyword` | mapped (by hand) |
| `correctness/noVoidTypeReturn` | — | covered by tsc / ES module strict mode |
| `suspicious/noDuplicateParameters` | — | covered by tsc / ES module strict mode |
| `suspicious/noImplicitAnyLet` | — | covered by tsc / ES module strict mode |
| `suspicious/noOctalEscape` | — | covered by tsc / ES module strict mode |
| `suspicious/noRedundantUseStrict` | — | covered by tsc / ES module strict mode |
| `a11y/noSvgWithoutTitle` | — | loss (no Oxlint rule) |
| `complexity/noEmptyTypeParameters` | — | loss (no Oxlint rule) |
| `complexity/noFlatMapIdentity` | — | loss (no Oxlint rule) |
| `complexity/noThisInStatic` | — | loss (no Oxlint rule) |
| `complexity/noUselessContinue` | — | loss (no Oxlint rule) |
| `complexity/noUselessStringRaw` | — | loss (no Oxlint rule) |
| `complexity/noUselessUndefinedInitialization` | — | loss (no Oxlint rule) |
| `complexity/useSimpleNumberKeys` | — | loss (no Oxlint rule) |
| `correctness/noConstantMathMinMaxClamp` | — | loss (no Oxlint rule) |
| `correctness/noStringCaseMismatch` | — | loss (no Oxlint rule) |
| `performance/noDynamicNamespaceImportAccess` | — | loss (no Oxlint rule) |
| `suspicious/noApproximativeNumericConstant` | — | loss (no Oxlint rule) |
| `suspicious/noMisrefactoredShorthandAssign` | — | loss (no Oxlint rule) |
| `suspicious/noSuspiciousSemicolonInJsx` | — | loss (no Oxlint rule) |
| `a11y/useGenericFontNames` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `complexity/noImportantStyles` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noDuplicateAttributes` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noInvalidDirectionInLinearGradient` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noInvalidGridAreas` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noInvalidPositionAtImportRule` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noMissingVarFunction` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownFunction` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownMediaFeatureName` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownPseudoClass` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownPseudoElement` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownTypeSelector` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnknownUnit` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noUnmatchableAnbSelector` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `style/noDescendingSpecificity` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDeprecatedMediaType` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDuplicateAtImportRules` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDuplicateCustomProperties` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDuplicateFontNames` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDuplicateProperties` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noDuplicateSelectorsKeyframeBlock` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noEmptyBlock` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noImportantInKeyframe` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noShorthandPropertyOverrides` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `suspicious/noUnknownAtRules` | — | loss (CSS/HTML: Oxlint lints JS/TS only) |
| `correctness/noDuplicateArgumentNames` | — | n/a (GraphQL, unused here) |
| `correctness/noDuplicateInputFieldNames` | — | n/a (GraphQL, unused here) |
| `correctness/noDuplicateVariableNames` | — | n/a (GraphQL, unused here) |
| `correctness/useGraphqlNamedOperations` | — | n/a (GraphQL, unused here) |
| `correctness/useLoneAnonymousOperation` | — | n/a (GraphQL, unused here) |
| `style/useDeprecatedReason` | — | n/a (GraphQL, unused here) |
| `suspicious/noDuplicateFields` | — | n/a (GraphQL, unused here) |
| `suspicious/noBiomeFirstException` | — | n/a (Biome meta) |
| `suspicious/noQuickfixBiome` | — | n/a (Biome meta) |
| `suspicious/useBiomeIgnoreFolder` | — | n/a (Biome meta) |

</details>

## Task 2

**Verdict: the overlay works for D1 and R2, and the ports work locally on Miniflare. An explicit DLQ binding duplicates the queue consumer, so the overlay must replace Void's inferred consumer, not add one. Provisioning, `void.lock.json` and rollback are blocked:** they need the owner's test account and tokens (§70 §12.2). Run on 2026-10-24 with `void` 1.2.1, `vite` 8.3.4 and Node 24.21.0. Nothing was deployed.

**The app.**
- A portable Void app (`target: "cloudflare"`) declares three ports in `loams.app.json`: `items` (SQL), `blobs`, and `queue` (`events`, `dead_letter: true`).
- A stand-in for the `@loams/app` Cloudflare adapter reads the bindings `LOAMS_ITEMS` (D1), `LOAMS_BLOBS` (R2) and `LOAMS_QUEUE` (a Queues producer).
- The adapter sits outside Void's scan directories, as it would in `node_modules`.
- `POST /api/items` writes an item, a blob and a message. `GET /api/items` reads the item and the blob back. The `queues/events.ts` consumer records the ids it sees.

**What Void infers without the overlay.**
- No D1 and no R2 binding. Inference reads only the scanned directories, so the adapter's `env[...]` reads are invisible to it.
- `void info --json` lists no resources.
- The `queues/` directory alone yields the producer `QUEUE_EVENTS` and a consumer for `events`.

**With the generated overlay** (`gen-overlay.mjs`, which stands in for `loams app deploy --target cloudflare`, D1779), `void.config.ts` imports the author's `void.config.base.ts` and layers on:
- `inference.bindings: { db: "LOAMS_ITEMS", storage: "LOAMS_BLOBS" }`;
- `cloudflare.queues: { producers: [{ queue: "events", binding: "LOAMS_QUEUE" }], consumers: [{ queue: "events", dead_letter_queue: "events-dlq" }] }`.

What happens:
- **D1 and R2 work.** The generated `dist/ssr/wrangler.json` carries `d1_databases: [{binding: "LOAMS_ITEMS", database_name: "default", database_id: "local"}]` and `r2_buckets: [{binding: "LOAMS_BLOBS", bucket_name: "default"}]`.
- **Queues are appended, not merged.** The result has two producers (`QUEUE_EVENTS`, which Void inferred, and `LOAMS_QUEUE`) and **two consumers for `events`**: Void's `{queue: "events"}` and ours with the DLQ. A Cloudflare queue takes one consumer. The extra producer is harmless.
  - **The fix for D1779:** the overlay names the port's queue with a different consumer file, or `loams app deploy` makes the consumer's settings Void's. The other option is that Void's inferred consumer reads `maxRetries` and the like from named exports (it already does for retries). Void has no `deadLetterQueue` export.
  - **To verify on a real deploy:** what Cloudflare does with the duplicate, and the merge rule this sets.
- **`void info --json` still lists no resources before a deploy.** `void.lock.json` is written only by `void deploy`, so no lock exists locally.
- **`vite dev` (Miniflare) passes:**
  - `POST /api/items` returns `{"ok":true}`;
  - `GET /api/items?id=a` returns `{"item":{"x":1},"blob":"{\"x\":1}"}`;
  - `GET /api/seen` returns `{"seen":["a"]}`, so the message sent through `LOAMS_QUEUE` reached the `events` consumer.

**The DLQ answer** (from Void's source and docs in `node_modules`):
- Void-managed queues configure no dead-letter queue (docs, `guide/queues.md`).
- `cloudflare.queues.consumers[].dead_letter_queue` is in Void's config type. It is validated as a string, passed through to the generated Wrangler config, and compared on redeploy (`matchesQueueConsumer` in `platform-lifecycle`).
- `@void/deploy-cloudflare` has no code that creates the DLQ queue itself. So a DLQ **can be bound explicitly**.
- **Unproven without an account:** whether Cloudflare creates `events-dlq` on deploy, or the port's provisioning must create it first. §71's `dead_letter = true` should assume the second and create it.

**Blocked** (needs the owner's Cloudflare Free test account and a scoped token): `void deploy --platform cloudflare`, a second deploy, `void project rollback`, checking that Void provisions exactly the declared resources and records them in `void.lock.json`, and the duplicate-consumer behaviour on Cloudflare.

**Reproduce.**

```sh
cd spikes/vv0/task2   # node_modules: void@1.2.1, vite@8.3.4, node@24.21.0 (as Task 3)
node gen-overlay.mjs app
cd app && node node_modules/vite/bin/vite.js build    # see dist/ssr/wrangler.json
node node_modules/void/dist/cli/cli.mjs info --json
node node_modules/vite/bin/vite.js dev --port 38522    # then POST/GET /api/items, GET /api/seen
```

## Task 3

**Verdict: go locally for HTTP, cron and queue; the Lambda part is blocked** (it needs the owner's test account and tokens, §70 §12.2). Run on 2026-10-24 with `void` 1.2.1, `vite` 8.3.4, `@hono/node-server` 1.x and Node 24.21.0 (the `node` npm package). Nothing was deployed.

**The app.** A portable Void app with `target: "node"`, one route (`routes/api/hello.ts`, `GET` and `POST`), one cron (`crons/tick.ts`, `cron = '*/5 * * * *'`) and one queue (`queues/jobs.ts`; it acks `n >= 0` and retries `n < 0`). `vite build` takes 52 ms and writes `dist/ssr/app.js` (88.7 kB, 25.6 kB gzip), `dist/ssr/index.js` and one chunk per route, cron and queue.

**The export shapes (§79 §8's verify item).**
- `dist/ssr/app.js` has one public export: the Hono app as `default`, with `fetch(request, env, ctx)`. The other exports (`defineHandler`, `defineQueue`, `defineScheduled`) are single-letter minified aliases. They are internal.
- **No `scheduled` or `queue` handler is exported** on the `node` target. Void compiles both into `app.js` as internal functions, and reaches them only through two HTTP routes on the same app: `POST /__void/scheduled` (`{cron, scheduledTime}`) and `POST /__void/queue` (`{queue, messages: [{id, timestamp, body, attempts}]}`, which returns `{ok, decisions: {<id>: {action: "ack" | "retry", delaySeconds?}}}`).
- **Each cron chunk** exports `cron` (a string or an array) and the handler as `default`, called as `default(controller, env, ctx)` with `controller = {cron, scheduledTime, noRetry()}`. **Each queue chunk** exports the handler as `default`, called as `default(batch, env)`. The chunk file names are hashed. `dist/ssr/.vite/manifest.json` maps each source path (`crons/tick.ts`, `queues/jobs.ts`) to its chunk.
- **The trigger routes' guard on `node`.** They accept the header `x-void-internal` only when it equals `__VOID_PROXY_TOKEN` from the raw environment. The `node` build never fills that store (`AsyncLocalStorage` with no `run`), so the token falls back to `__VOID_BUILD_KEY`. That is a random UUID, new on every build, and stored as plain text in `app.js`. `CRON_SECRET` is not read on this target.
- **`POST /__void/scheduled` is broken on `node`.** Under `@hono/node-server` it reads `c.executionCtx`, which throws "This context has no ExecutionContext", so it answers 500. `POST /__void/queue` works with the build key.
- **`dist/ssr` is not self-contained.** `app.js` imports `@hono/node-server/serve-static` and leaves it external, and that package's peer `hono` has to sit beside it too. The deploy artefact therefore carries `node_modules/@hono/node-server` and `node_modules/hono` (3.4 MB in all, the build included).

**The shim (spike, `spikes/vv0/task3/shim/lambda.mjs`, 140 lines).** One `handler(event, context)` takes three event shapes:
- **A Function URL event** (payload 2.0). It becomes a `Request`: path, query, headers, cookies, and a base64 or text body. `app.fetch(request, process.env, ctx)` answers it, and the `Response` maps back to `{statusCode, headers, cookies, body, isBase64Encoded}`. The shim answers `/__void/*` itself with 404, because the `node` build mounts the trigger routes on the public app.
- **An EventBridge Scheduler invocation** whose custom input is `{"loams": "schedule", "cron": "<expr>", "time": "<ISO>"}`. The shim imports every `crons/` chunk listed in the manifest, runs those whose `cron` export includes the expression, and throws an `AggregateError` if any fails. That is the same rule as Void's Workers `scheduled`. So the §71 `schedule` port provisions one Scheduler schedule per expression, with that input.
- **An SQS batch.** The queue name is the last part of `eventSourceARN`, matched to a `queues/` chunk by exact name, or else by the longest `-<name>` suffix that is unambiguous (Void's own rule, so a prefixed physical queue still resolves). The shim builds Void's batch (`messages[].ack()` and `retry()`, `ackAll()`, `retryAll()`) and returns `{batchItemFailures}`. It lists each message that was retried, or that had no decision when the handler threw. Records for an unknown queue all fail. **One loss:** `retry({delaySeconds})` becomes a plain redelivery after the visibility timeout; honouring the delay would need `ChangeMessageVisibility`.
- **The shim calls the chunks directly, not the trigger routes.** On `node` the routes are broken (crons) or guarded by a per-build key read out of the bundle (queues), and they are public.

**Tests.**
- `node --test shim/lambda.test.mjs` on Node 24.21.0: **7 passed.** They cover GET; a POST with a base64 body; the app's 404; the blocked `/__void/scheduled`; a cron that runs by its expression (and an unmatched one, which runs nothing); an SQS batch of `[ack, retry, ack]`, which reports `batchItemFailures: [b]`; and a prefixed queue name, plus an unknown one.
- **The AWS Lambda Runtime Interface Emulator** (the public image `public.ecr.aws/lambda/nodejs:24`, 439 MB, pulled without an account). `/var/task` mounts `app/dist`, the shim and the two packages, with the handler `shim/lambda.handler`. The HTTP event answers 200 with the JSON body, the Scheduler input answers `{"ran":1}` and logs the tick, and the SQS batch answers `{"batchItemFailures":[{"itemIdentifier":"b"}]}`.

**Cold start, local only (not Lambda; load average 7 to 14 from other agents' builds).**

| Measure | Median | Runs |
|---|---|---|
| A fresh Node 24 process that imports the shim and answers one Function URL event | 72.8 ms (an empty `node -e ''` takes 24.7 ms) | 20 |
| RIE: `INIT REPORT` for a fresh container | 68 ms | 7 |
| RIE: first invoke, from the client | 122 ms | 7 |
| RIE: the first container ever, with a cold disk cache | 1.6 s init | 1 |
| RIE: a warm invoke | 2 to 4 ms | — |

Real Lambda cold starts (Init Duration behind a Function URL) are **blocked**, and so are the EventBridge Scheduler and SQS event-source mappings: they need the owner's AWS credit account and a scoped token. Nothing here changes Q1311's answer (Node on Lambda) yet.

**Upstream notes** (not filed; no upstream posts without consent):
- `node`'s `/__void/scheduled` crashes on the missing `executionCtx`.
- The trigger key falls back to a per-build constant kept in the bundle.
- An exported `scheduled` and `queue` on the `node` target would let shims skip the manifest.

**Reproduce.**

```sh
cd spikes/vv0/task3
npm install -D void@1.2.1 vite@8.3.4 node@24.21.0 @hono/node-server@1
npm install-scripts approve node workerd esbuild better-sqlite3 && npm rebuild
(cd app && PATH=$PWD/../node_modules/.bin:$PATH node ../node_modules/vite/bin/vite.js build)
node_modules/.bin/node --test shim/lambda.test.mjs
node_modules/.bin/node cold.mjs
podman run --rm -p 39123:8080 -v $PWD/task:/var/task:ro,Z public.ecr.aws/lambda/nodejs:24 shim/lambda.handler
curl -XPOST localhost:39123/2015-03-31/functions/function/invocations -d '<event>'
```

npm 11 blocks install scripts by default, which is why the approve step is there. Without it the `node` package has no binary and `workerd` isn't fetched.

## Task 4

**Verdict: go.** The same portable Void app was built two ways and ran side by side in a compose stack, on Postgres and an S3 bucket through `env.native`. It served pages, hydrated and kept its data across both an app restart and a whole-stack restart. The two builds were the Bun-compiled binary on distroless (138.9 MB) and the `node` fallback on `node:24-slim` (247.3 MB). `pg` and an S3 client (`aws4fetch`) ran unchanged under the compiled binary.

### What is missing, so what was used instead

- **`loams app build` doesn't exist yet** (AB0). `spikes/vv0/task4/build.sh <bun-compile|node> <app> <tag>` emulates `loams app build --target bun --compile --image` and `--target node --image`.
- **`deploy/release/compose.yaml` has no `apps` profile yet.** The spike's own `compose.yaml` (project `vv0task4`) gives the two app services `profiles: [apps]`, beside two data services:
  - `postgres:17`, standing in for Loams Postgres (the same wire protocol; Loams Postgres's storage stack was not started);
  - `rustfs/rustfs:1.0.0`, the image of the light stack's `rustfs` profile (pinned by digest, as in `deploy/fabric/compose.yaml`).

  Host ports are bound on loopback only, at 25532, 29100, 23101 and 23102.
- **`@loams/app` doesn't exist yet.** The app's `lib/env.ts` stands in for its ports:
  - `env.items` on Postgres through `pg` 8.16.3;
  - `env.blobs` on the bucket through `aws4fetch` 1.0.20 (MIT);
  - the clients themselves on `env.native.pg` and `env.native.s3`, as D1393 describes.

  The configuration comes from `LOAMS_NATIVE_PG_URL` and `LOAMS_NATIVE_S3_{ENDPOINT,BUCKET,ACCESS_KEY,SECRET_KEY}`. These names are the spike's proposal for what the `apps` profile injects. The real contract is AB0's to define.

### The app

The Task 8 notes skeleton, with state:
- `POST /api/notes` writes the body to the bucket (`notes/<uuid>.txt`) and a row to Postgres (`notes(id, text, blob_key)`);
- `GET /api/notes/:id` reads the row and the body back;
- `GET /` is an SSR page that lists the notes;
- `GET /api/health` reports the runtime.

The pins are the same as in Task 8: `void` 1.2.1, Bun 1.4.4, `vite` 8.3.5.

### The builds (emulating `loams app build`)

- **`bun-compile`:**
  1. `VOID_TARGET=bun vite build`;
  2. copy `dist/` into `<app>/.loams-build/`, inside the app, so that `bun build` resolves the app's `node_modules`. Compiling from a directory outside the app fails with `Could not resolve: "react"`, because Void leaves `react`, `react-dom`, `hono`, `pg` and `aws4fetch` as bare imports;
  3. rewrite Void's client root (Task 8), and **fail the build if the pattern isn't found**;
  4. append a SIGTERM handler;
  5. `bun build ./ssr/index.js --asset=./client --compile --bytecode --minify --sourcemap --format=esm --target=bun-linux-x64`;
  6. `FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79…`, `COPY app /app`, with the label `io.loams.app.runtime="bun-compiled 1.4.4"`.

  The image is **138.9 MB**.
- **`node`:**
  1. `VOID_TARGET=node vite build`;
  2. the SIGTERM handler;
  3. `npm ci --omit=dev` (the production `node_modules`);
  4. `FROM node:24-slim@sha256:55fb5f5a…`, running as `USER node`, with the label `io.loams.app.runtime="node 24"`.

  The image is **247.3 MB**.

The `io.loams.app.runtime` label is how "the deploy records which runtime it used" (§79 §7.6) could be read back from the image.

### Results

| Check | Bun compiled | Node |
|---|---|---|
| `GET /api/health` | `{"ok":true,"runtime":"bun 1.4.4"}` | `{"ok":true,"runtime":"node v24.22.1"}` |
| `POST /api/notes`, then the SSR page lists it | yes | yes |
| `<script src="/assets/pages-client-….js">` in the page, and the asset served with a 200 | yes (embedded) | yes |
| `compose restart` of the app, then the row and the bucket body read back | **persisted** | **persisted** |
| Time from `compose restart` to the first 200 | 273 ms | 379 ms |
| `compose stop` of the whole stack, then `up -d`, then the row and the body read back | **persisted** | **persisted** |
| Memory at idle after one request (cgroup, `podman stats`) | 13.0 MB | 39.9 MB |

Both apps shared one Postgres (databases `notes_bun` and `notes_node`) and one RustFS (buckets `notes-bun` and `notes-node`, created by the app on first use). Postgres was reached through the compose network (`postgres:5432`), and RustFS as `http://rustfs:9000` with path-style requests.

### Findings for AB0 and VV2

- **`loams app build --target bun --compile` must compile inside the app directory,** or give `bun build` the app's `node_modules`. Void's `dist/ssr` isn't self-contained: it leaves the app's `dependencies` as bare imports. The same fact is why the `node` and `bun run` images need `node_modules` and the compiled image doesn't.
- **The client-root rewrite needs a guard,** because a Void upgrade can move the pattern. Without the rewrite the failure is silent: the page renders with no client script (Task 8). The guard has to stay until Void has an option for the client root.
- **PID 1 and SIGTERM.** Neither Void entry handles SIGTERM. Without the appended handler, `compose stop` waits out its timeout and then kills the app. The build adds the handler, or the image runs with an init process.
- **`env.native` covers what this task needs.** Passing the native clients' configuration as environment, and building the clients inside the app, works on both runtimes with no Bun-specific code. Bun's own `Bun.sql` and `Bun.s3` were deliberately not used, so the `node` fallback stays the same code.
- **Not covered here:**
  - the engine's own bucket (`file:///var/lib/loams/bucket`) and Loams Postgres itself;
  - the tailnet exposure of an app in the real stack;
  - adding an `apps` profile to `deploy/release/compose.yaml`.

  These belong to AB0 and VV2, and the profile change is a release-compose PR.

### Reproduce

Spike code: `spikes/vv0/task4/` (`app/`, `build.sh`, `compose.yaml`, `initdb/01.sql`, `test.sh`), not committed.

```sh
cd spikes/vv0/task4/app && npm i && (cd node_modules/bun && node install.js) && npx void prepare && cd ..
./build.sh bun-compile app localhost/vv0-task4-notes:bun-compiled
./build.sh node        app localhost/vv0-task4-notes:node
podman compose -f compose.yaml --profile apps up -d
./test.sh                    # serve, write, restart each app, read back
podman compose -f compose.yaml --profile apps stop && podman compose -f compose.yaml --profile apps up -d
curl -s 127.0.0.1:23101/api/notes/1; curl -s 127.0.0.1:23102/api/notes/1
podman compose -f compose.yaml --profile apps down -v
```

**Cleanup:** `podman compose down -v` removed the spike's containers, network and both volumes (`vv0task4_pg` and `vv0task4_bucket`). The two app images were removed with `podman rmi`, and the build stages and the app's `node_modules` were deleted. The base images (`postgres:17`, `rustfs:1.0.0`, `node:24-slim` and `distroless/cc-debian12:nonroot`) are kept: the first two were already present for other stacks.

## Task 5

Pending.

## Task 6

**Question.** Can the whole workspace take one `vite-plus` pin, with `vite` aliased?

**Verdict: go.** Every JavaScript target is green on one pin. The design's "the peer warnings disappear" holds only with one more pnpm setting. The desktop builds on `vite-plus-core` through electron-vite 6.0.0, with the same bundle as PR #1049.

**The change tried (2026-10-24, `vite-plus` 1.3.0):**
- **`pnpm-workspace.yaml`:**
  - catalog entries `vite-plus: 1.3.0`, `vite: npm:@voidzero-dev/vite-plus-core@1.3.0` and `vitest: 5.0.5`;
  - the four `vite-plus@1.0.0>…` and `vitest@5.0.1>…` scoped overrides replaced by two global ones, `vite: npm:@voidzero-dev/vite-plus-core@1.3.0` and `vitest: 5.0.5`.
- **Every `package.json`** that named `vite`, `vitest` or `vite-plus` now says `catalog:`. That is 51 packages, plus `vite-plus` added to the console, `@loams/ui`, the desktop and `web/`.
- **Two version bumps:** `@vitest/coverage-v8` 5.0.3 to 5.0.5 in `plugins/`, and electron-vite 5.0.0 to 6.0.0 in the desktop.
- **Task 1's script changes** for the console and `@loams/ui`.

### Results

| Check | Result |
|---|---|
| `pnpm install` | Clean. The lockfile is 689 lines shorter (15,419 to 14,730). Before: `vite` 7.3.7 and 8.3.1, `vite-plus-core` 1.0.0, `vitest` 5.0.1 and 5.0.3, and `esbuild` 0.25.12. After: one `vite-plus-core` 1.3.0 (which reports Vite 8.3.4 in `bundledVersions`), one `vitest` 5.0.5, and no `esbuild` 0.25 |
| Peer warnings, without a rule | **4 unmet `vite` peers**, all for the same reason. `@voidzero-dev/vite-plus-core` identifies as its own version, 1.3.0, so ranges such as `^8.0.0` fail. The packages asking are `vitest` 5.0.5, `@vitest/mocker` 5.0.5, `@vitejs/plugin-react` 6.1.2, `@tailwindcss/vite` 4.3.3 and electron-vite 6.0.0. `dev` today has 5 unmet peers of its own: 3 for `vite` (the plugins' fork, which identifies as 1.0.0), one for `@vitest/coverage-v8` and one for `@vitest/browser-preview` |
| Peer warnings, with `peerDependencyRules.allowedVersions: { vite: 1.3.0 }` | `pnpm peers check`: "No peer dependency issues found". This rule moves with the pin, so a bump changes it in the same line |
| `nx run-many --targets=build,test,typecheck --projects='@loams/*,plugins*,loams-desktop-electron' --parallel=2 --skip-nx-cache` | **171 tasks in 75 projects, all green**, in 6 min 22 s, with `NX_NO_CLOUD=true` so nothing reached the remote cache |
| The desktop's `electron-vite build` (6.0.0 on `vite-plus-core` 1.3.0) | Green in 9 s. `out/main/index.js` is 2,065,819 bytes, with no `getElectronPath`: the same as #1049's 2,066.14 kB with plain Vite 8.3.4. So electron-vite 6 runs on `vite-plus-core`, and §79 §5.5's "until electron-vite runs on `vite-plus-core`" is already true. The desktop's unit tests passed in the run-many above. Its Playwright smoke wasn't run here, because it opens Electron windows on the shared machine's display. The VV1 PR runs it in CI |
| Nx | Unchanged. The targets keep their names, and `vp` runs inside them (Q1302) |

### Inputs to VV1

- **The 14-day rule decides which pin lands first.**
  - `vite-plus` 1.3.0 was published on 2026-10-16, so it is eligible on 2026-10-30. 1.2.0 (2026-10-14) is eligible on 2026-10-28.
  - **1.1.0** (2026-10-07) is eligible now. It bundles Vite 8.3.3, Rolldown 1.2.12 and tsdown 0.23.0, and it depends on Vitest 5.0.3, the version the workspace already uses.
  - electron-vite 6.0.0 (2026-10-14) is eligible on 2026-10-28. Until then, the desktop can't take the global `vite` alias: electron-vite 5 on Vite 8 inlines Electron's launcher, as #1049 found.
  - So the first VV1 pin is `vite-plus` 1.1.0 through the catalog for every package except the desktop, without a global `vite` override. The desktop keeps Vite 7.3.7 and electron-vite 5.0.0 until 2026-10-28, and then takes #1049's electron-vite 6.0.0 and the global alias.
- **The `vite` binary goes away.** Every script that runs `vite` becomes `vp` in the same PR (see Task 1).
- **`docs/monorepo.md`'s "Do not apply a global Vite override"** is replaced in that PR by the catalog entry and the `peerDependencyRules` line.

**Reproduce.** In a worktree off `dev`:
1. Apply the catalog entries, the overrides and the `peerDependencyRules` above, and Task 1's script changes.
2. Run `pnpm install --no-frozen-lockfile` and `pnpm peers check`.
3. Run `NX_NO_CLOUD=true NX_DAEMON=false pnpm exec nx run-many --targets=build,test,typecheck --projects='@loams/*,plugins*,loams-desktop-electron' --parallel=2 --skip-nx-cache`.
4. Run `pnpm --filter loams-desktop-electron run build`.

## Task 7

Pending.

## Task 8

**Verdict: go for the cold start, no-go for the image-size target as written.** A Bun-compiled Void app reaches its first 200 about **5.5x faster** than the same app on Node 24, against a target of at least 1.5x. Its image is **138 MB** on distroless and **118 MB** on Alpine, against a target of under 120 MB. The Bun 1.4.4 runtime alone is 109 MB, not the 77 MB §79 assumed. One Void path needs a one-line rewrite before `dist/client/` can be embedded (see "Static assets").

### What was built

- **The app.** There is no `web/examples/app-byoc-notes` yet, so the spike used a notes skeleton made with `void init --framework react --starter static --deploy none --toolchain vite`:
  - one SSR page with a loader (`pages/index.tsx` and `pages/index.server.ts`);
  - `routes/api/notes.ts` (GET and POST, in memory) and `routes/api/health.ts`;
  - no `void/*` binding imports, so the app is portable.

  `void.config.ts` sets `target` from `VOID_TARGET`.
- **Pins:** `void` 1.2.1, `@void/react` 1.2.1, `bun` 1.4.4 (from npm; its `install.js` was run by hand because npm 11 blocks install scripts), `hono` 4.13.13, React 19.3.1, and `vite` 8.3.5 (`void init` resolved `^8.0.10` to it).
- **Node:** builds ran on the host's Node 26.8.2, which meets Void's `>=24.21.0`. Measurements used Node 24.22.1, copied from `node:24-slim` (digest `sha256:55fb5f5a…`), with Node 26.8.2 as a reference.
- **The three builds:**
  1. `VOID_TARGET=node vite build` (2.1 s), run as `node dist/ssr/index.js` on `node:24-slim`, with production `node_modules` (12 MB: `hono`, `@hono/node-server`, `react`, `react-dom`);
  2. `VOID_TARGET=bun vite build` (1.2 s), run as `bun dist/ssr/index.js`;
  3. the same output, compiled with `bun build ./ssr/index.js --compile --bytecode --minify --sourcemap --format=esm --target=bun-linux-x64` (5.3 s cold, under 0.4 s warm), on `gcr.io/distroless/cc-debian12:nonroot`.

### Numbers

These are the median of 20 runs: spawn to the first 200 on `GET /` (an SSR page), polled every 2 ms. RSS is the median of 5 runs, read 2 s after the first 200. The host is a 14-core x86-64 machine shared with other build agents, with a load average of 1.5 to 6 during the runs. Each figure was repeated in a second round and agreed within 1 ms.

**The process on the host:**

| Runtime | Cold start (median) | Min | Speed-up against Node 24 | RSS at idle |
|---|---|---|---|---|
| Node 24.22.1, `target: "node"` | 96.0 ms (95.2 ms in round 2) | 82–86 ms | 1.0x | 71.5 MB |
| Node 26.8.2, `target: "node"` | 92.0 ms | 82.9 ms | 1.04x | 70.0 MB |
| `bun run`, `target: "bun"` | 46.0 ms | 41.0 ms | 2.1x | 39.7 MB |
| Bun compiled, no `--bytecode` | 40.9 ms | 36.7 ms | 2.3x | 42.3 MB |
| **Bun compiled, `--bytecode`, client embedded** | **17.6 ms** (17.5 ms) | 16.0 ms | **5.5x** | 34–41 MB |
| Bun compiled, `--bytecode --bytecode-order` | 17.9 ms | 16.3 ms | 5.4x | 39.8 MB |
| Bun compiled, `--bytecode`, client beside the binary | 20.4 ms | 16.3 ms | 4.7x | 34.3 MB |

On `GET /api/health` (no SSR), Node 24 took 80.6 ms and the compiled binary took 15.5 ms, a 5.2x speed-up.

**The images**, started with `podman run --rm --init --network host` (podman 6.1.1, rootless):

| Image | Base | Size | Cold start to the first 200 | Memory at idle (cgroup) |
|---|---|---|---|---|
| Node | `node:24-slim` | 247 MB | 218.5 ms | 25.6 MB |
| `bun run` | distroless `cc-debian12`, the `bun` binary and `node_modules` | 143 MB | 190.1 ms | 14.7 MB |
| **Bun compiled (glibc)** | distroless `cc-debian12` | **138 MB** | **148.1 ms** | 9.6 MB |
| Bun compiled (musl) | `alpine:3.22` with `libstdc++` and `libgcc` | **118 MB** | 148.0 ms | 9.6 MB |

- Inside a container, podman's own start-up (about 130 ms) dominates. The process-level gain is the one a long-lived container, a scale-from-zero container or a CLI sees.
- The binary is 112.7 MB uncompressed and 53.6 MB gzipped. The musl binary is 106.3 MB uncompressed and 50.1 MB gzipped. Registries store gzipped layers, so the pull is about 55 MB.
- The external source map (`--sourcemap`) is 1.6 MB and goes beside the binary. It isn't part of the image figures.

### Against §79 §7.6's targets

- **Cold start at least 1.5x faster: met.** The speed-up is 5.5x for the process, and 1.5x for the whole container (218.5 ms against 148.1 ms). `--bytecode` is what gives most of it: without it, the compiled binary is only 2.3x faster.
- **An image under 120 MB: not met on glibc (138 MB), met on musl (118 MB).** Bun 1.4.4's runtime is 109 MB (a compiled empty script is 109,123,760 bytes). That puts any glibc image above 130 MB, before any code. The proposed change, for the owner: restate the target as "smaller than the Node image" (138 MB against 247 MB is 44% smaller), or as "under 60 MB compressed". The alternative is the musl build on Alpine as the default.
- **`--bytecode-order` gains nothing measurable for an app this small** (17.9 ms against 17.6 ms). The order file is written only on a clean `process.exit()`. Void's generated `index.js` has no signal handler, so the spike added `process.on("SIGTERM", () => process.exit(0))` before compiling. That handler is also needed in a container in any case, because PID 1 ignores SIGTERM by default. Recommendation: skip `--bytecode-order` until an app's start-up is large enough to measure it.

### Static assets (§79 §7.6's verify item)

- **Void's static middleware reads from disk.** The generated `.void/app.ts` serves `dist/client/` with `serveStatic` from `hono/bun`, at `fileURLToPath(new URL("../../client/", import.meta.url))`. Inside a compiled binary, `import.meta.url` is `file:///$bunfs/root/<name>`, so the root becomes the absolute path `/client/`.
- Void also fetches `/.vite/manifest.json` through the same middleware to write the page's `<script>` tags. When the files aren't found, pages still render with a 200, but with no client script: **the page is never hydrated, and nothing fails loudly.** This is the failure to test for.
- **Beside the binary: works, but only at `/client/`.** The image copies `dist/client/` to `/client/` at the filesystem root. It doesn't work at any path relative to the binary.
- **Embedded with `--asset`: works after a one-line rewrite.** `--asset=./client` embeds the files at `/$bunfs/root/client/…`, and `Bun.file` and `fs.existsSync` read them. Before compiling, the build replaces `new URL("../../client/", import.meta.url)` with `new URL("./client/", import.meta.url)` in `dist/ssr/assets/app-*.js`. After that, `/`, the hashed JavaScript and the manifest are all served from the binary.
- **Recommendation for `loams app build --target bun --compile`: embed.** It gives one file and no layout rule. The rewrite is a text match on Void's output, so the build must fail when the pattern isn't found. The upstream fix would be an option or environment variable for the client root. Don't file it upstream without the owner's consent.
- **Side finding.** `dist/client/.vite/manifest.json` is publicly served on every target, Node included. It lists only hashed asset names. It's worth a note, but it isn't a blocker.

### Cross-compiling

- `--target=bun-linux-arm64`, `bun-linux-x64-musl` and `bun-linux-arm64-musl` all built on the x64 host. Bun downloaded each target's runtime from npm (`bun-linux-aarch64-v1.4.4` and the others).
- The sizes are 112.8 MB (arm64), 106.3 MB (x64 musl) and 105.3 MB (arm64 musl).
- The arm64 binaries were not run, because the host has no qemu-user or binfmt support. Running them is left to an arm64 CI runner.
- The x64 musl binary ran in `alpine:3.22` once `libstdc++` and `libgcc` were added. Without them it doesn't start.

### What fails, and the fallbacks

- **Observed:** only the static-asset path above. `hono`, `react-dom/server` and Void's runtime all compiled and ran unchanged.
- **Not exercised, in a portable notes skeleton:** native addons and packages that read files beside their own code. Void's own `better-sqlite3` and `@napi-rs/keyring` are build-time dependencies and aren't in `dist/ssr`. Task 4 exercises `pg` and an S3 client under the compiled binary.
- **The fallbacks, in order:**
  1. `bun run dist/ssr/index.js` on distroless with `node_modules` (2.1x faster than Node, 143 MB);
  2. `target: "node"` on `node:24-slim` (247 MB).

  The deploy records which runtime it used (§79 §7.6).

### The LGPL-2 notice (draft for Q1312, for counsel; nothing ships)

No binary is added to any release artefact. This is the text proposed to ship as `THIRD_PARTY_NOTICES/bun-runtime.txt` beside, or inside the image with, every Bun-compiled binary that Loams distributes, once counsel has reviewed it:

```
This program is a Bun single-file executable. It contains the Bun runtime
<BUN_VERSION> (https://github.com/oven-sh/bun, MIT), which includes
JavaScriptCore and other parts of WebKit, licensed under the GNU Lesser
General Public License version 2 (LGPL-2.0) and BSD-style licences.

The WebKit source for this build is at
https://github.com/oven-sh/WebKit/tree/<WEBKIT_COMMIT>, the commit pinned by
Bun <BUN_VERSION>. A copy of the LGPL-2.0 is in LICENSE.LGPL-2.0 beside this
notice.

This program's own source is Apache-2.0 and is at
https://github.com/ostrium-labs/loams/tree/<LOAMS_COMMIT>/<APP_PATH>. To
rebuild or relink it against a modified JavaScriptCore, build Bun from source
with your modified WebKit, then run:

  vite build   (with target "bun" in void.config.ts)
  bun build ./dist/ssr/index.js --compile --bytecode --minify --format=esm \
    --asset=./dist/client --target=<TARGET> --outfile <NAME> \
    --compile-executable-path=<your bun>

Bytecode in this program is tied to Bun <BUN_VERSION>. A different runtime
rebuilds it on first start.
```

Questions for counsel:
- whether `--compile-executable-path` relinking satisfies LGPL-2 §6 for a statically embedded JavaScriptCore;
- whether the WebKit source pointer must be an offer of physical source, or a URL is enough;
- whether the notice must be inside the binary (`--asset`) as well as beside it.

### Reproduce

Spike code: `spikes/vv0/task8/`, not committed. From an empty directory:

```sh
npm i --save-exact void@1.2.1 bun@1.4.4 && (cd node_modules/bun && node install.js)
mkdir scaffold && cd scaffold && echo '{"name":"notes-skel","private":true,"type":"module"}' > package.json
../node_modules/.bin/void init --framework react --starter static --deploy none --toolchain vite --no-git
npm i --save-exact @hono/node-server
# add routes/api/notes.ts, routes/api/health.ts, pages/index.server.ts; void.config.ts: target from VOID_TARGET
VOID_TARGET=node npx vite build && mv dist dist-node
VOID_TARGET=bun  npx vite build && mv dist dist-bun
cp -r dist-bun emb
sed -i 's#new URL("../../client/", import.meta.url)#new URL("./client/", import.meta.url)#' emb/ssr/assets/app-*.js
printf '\nprocess.on("SIGTERM", () => process.exit(0));\n' >> emb/ssr/index.js
(cd emb && ../../node_modules/.bin/bun build ./ssr/index.js --asset=./client --compile --bytecode \
  --minify --sourcemap --format=esm --target=bun-linux-x64 --outfile ../out/app-x64-embed)
# cold start: node harness/coldstart.mjs <label> 20 <port> / -- <command>
```

The images: `FROM gcr.io/distroless/cc-debian12:nonroot`, `COPY app /app` and `ENTRYPOINT ["/app"]`. The musl image uses `FROM alpine:3.22` with `apk add libstdc++ libgcc`.

**Cleanup:** the spike's images (`localhost/vv0-task8-*`) and its build directories were removed after the run. The base images `node:24-slim`, `distroless/cc-debian12:nonroot` and `alpine:3.22` were pulled for this task and are kept for Task 4.
