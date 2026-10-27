import { defineConfig } from 'vite-plus';

// Lint and format for web/ (design 79 D1773, plan VV1 item 3): Oxlint and Oxfmt
// through `vp check`, replacing Biome. The rule set is VV0 Task 1's parity table
// (docs/plans/vv0-report.md): each Biome rule of the `recommended` preset and the
// React domain mapped to its Oxlint rule (named in the trailing comment). Rules
// with no Oxlint counterpart, and Biome's CSS rules, are recorded losses there.
// Rules set to 'warn' fired on the code when this config landed (VV1, 2026-10-27);
// fix them over time and raise them to 'error'.
const generated = [
  'apps/console/src/api/schema.d.ts',
  'packages/proto/src/gen/**',
  'packages/agentd-client/src/gen/**',
  'packages/agentd-client/src/fixtures.gen.ts',
  'packages/agentd-client/src/methods.gen.ts',
  'packages/agentd-client/test/transcript.connect.gen.json',
  'packages/agentd-client/test/requests.json',
];

export default defineConfig({
  lint: {
    plugins: ['react', 'jsx-a11y', 'typescript', 'unicorn', 'oxc'],
    // Only the mapped rules below: Oxlint's default `correctness` set is wider than
    // Biome's preset, and widening the gate is a separate change.
    categories: { correctness: 'off' },
    ignorePatterns: ['**/dist/**', ...generated],
    rules: {
      'array-callback-return': 'error', // suspicious/useIterableCallbackReturn
      'constructor-super': 'error', // correctness/noInvalidConstructorSuper
      'default-case-last': 'error', // suspicious/useDefaultSwitchClauseLast
      eqeqeq: 'warn', // suspicious/noDoubleEquals
      'for-direction': 'error', // correctness/useValidForDirection
      'getter-return': 'error', // suspicious/useGetterReturn
      'jsx-a11y/alt-text': 'error', // a11y/useAltText
      'jsx-a11y/anchor-ambiguous-text': 'error', // a11y/noAmbiguousAnchorText
      'jsx-a11y/anchor-has-content': 'error', // a11y/useAnchorContent
      'jsx-a11y/anchor-is-valid': 'error', // a11y/useValidAnchor
      'jsx-a11y/aria-activedescendant-has-tabindex': 'error', // a11y/useAriaActivedescendantWithTabindex
      'jsx-a11y/aria-props': 'error', // a11y/useValidAriaProps
      'jsx-a11y/aria-proptypes': 'error', // a11y/useValidAriaValues
      'jsx-a11y/aria-role': 'error', // a11y/useValidAriaRole
      'jsx-a11y/aria-unsupported-elements': 'error', // a11y/noAriaUnsupportedElements
      'jsx-a11y/autocomplete-valid': 'error', // a11y/useValidAutocomplete
      'jsx-a11y/click-events-have-key-events': 'error', // a11y/useKeyWithClickEvents
      'jsx-a11y/heading-has-content': 'error', // a11y/useHeadingContent
      'jsx-a11y/html-has-lang': 'error', // a11y/useHtmlLang
      'jsx-a11y/iframe-has-title': 'error', // a11y/useIframeTitle
      'jsx-a11y/img-redundant-alt': 'error', // a11y/noRedundantAlt
      'jsx-a11y/interactive-supports-focus': 'warn', // a11y/useFocusableInteractive
      'jsx-a11y/label-has-associated-control': 'warn', // a11y/noLabelWithoutControl
      'jsx-a11y/lang': 'error', // a11y/useValidLang
      'jsx-a11y/media-has-caption': 'error', // a11y/useMediaCaption
      'jsx-a11y/mouse-events-have-key-events': 'error', // a11y/useKeyWithMouseEvents
      'jsx-a11y/no-access-key': 'error', // a11y/noAccessKey
      'jsx-a11y/no-aria-hidden-on-focusable': 'error', // a11y/noAriaHiddenOnFocusable
      'jsx-a11y/no-autofocus': 'warn', // a11y/noAutofocus
      'jsx-a11y/no-distracting-elements': 'error', // a11y/noDistractingElements
      'jsx-a11y/no-interactive-element-to-noninteractive-role': 'error', // a11y/noInteractiveElementToNoninteractiveRole
      'jsx-a11y/no-noninteractive-element-to-interactive-role': 'error', // a11y/noNoninteractiveElementToInteractiveRole
      'jsx-a11y/no-noninteractive-tabindex': 'warn', // a11y/noNoninteractiveTabindex
      'jsx-a11y/no-redundant-roles': 'error', // a11y/noRedundantRoles
      'jsx-a11y/no-static-element-interactions': 'warn', // a11y/noStaticElementInteractions
      'jsx-a11y/prefer-tag-over-role': 'warn', // a11y/useSemanticElements
      'jsx-a11y/role-has-required-aria-props': 'error', // a11y/useAriaPropsForRole
      'jsx-a11y/role-supports-aria-props': 'warn', // a11y/useAriaPropsSupportedByRole
      'jsx-a11y/scope': 'error', // a11y/noHeaderScope
      'jsx-a11y/tabindex-no-positive': 'error', // a11y/noPositiveTabindex
      'no-array-constructor': 'error', // style/useArrayLiterals
      'no-async-promise-executor': 'error', // suspicious/noAsyncPromiseExecutor
      'no-case-declarations': 'error', // correctness/noSwitchDeclarations
      'no-class-assign': 'error', // suspicious/noClassAssign
      'no-compare-neg-zero': 'error', // suspicious/noCompareNegZero
      'no-cond-assign': 'error', // suspicious/noAssignInExpressions
      'no-const-assign': 'error', // correctness/noConstAssign
      'no-constant-condition': 'error', // correctness/noConstantCondition
      'no-constructor-return': 'error', // correctness/noConstructorReturn
      'no-control-regex': 'error', // suspicious/noControlCharactersInRegex
      'no-debugger': 'error', // suspicious/noDebugger
      'no-dupe-class-members': 'error', // suspicious/noDuplicateClassMembers
      'no-dupe-else-if': 'error', // suspicious/noDuplicateElseIf
      'no-dupe-keys': 'error', // suspicious/noDuplicateObjectKeys
      'no-duplicate-case': 'error', // suspicious/noDuplicateCase
      'no-empty-character-class': 'error', // correctness/noEmptyCharacterClassInRegex
      'no-empty-pattern': 'error', // correctness/noEmptyPattern
      'no-eval': 'error', // security/noGlobalEval
      'no-ex-assign': 'error', // suspicious/noCatchAssign
      'no-extra-boolean-cast': 'error', // complexity/noExtraBooleanCast
      'no-extra-label': 'error', // complexity/noUselessLabel
      'no-fallthrough': 'warn', // suspicious/noFallthroughSwitchClause
      'no-func-assign': 'error', // suspicious/noFunctionAssign
      'no-global-assign': 'error', // suspicious/noGlobalAssign
      'no-import-assign': 'error', // suspicious/noImportAssign
      'no-inner-declarations': 'error', // correctness/noInnerDeclarations
      'no-irregular-whitespace': 'error', // suspicious/noIrregularWhitespace
      'no-label-var': 'error', // suspicious/noLabelVar
      'no-labels': 'error', // suspicious/noConfusingLabels
      'no-lone-blocks': 'error', // complexity/noUselessLoneBlockStatements
      'no-loss-of-precision': 'error', // correctness/noPrecisionLoss
      'no-misleading-character-class': 'error', // suspicious/noMisleadingCharacterClass
      'no-new-native-nonconstructor': 'error', // correctness/noInvalidBuiltinInstantiation
      'no-nonoctal-decimal-escape': 'error', // correctness/noNonoctalDecimalEscape
      'no-obj-calls': 'error', // correctness/noGlobalObjectCalls
      'no-proto': 'error', // suspicious/noProto
      'no-prototype-builtins': 'error', // suspicious/noPrototypeBuiltins
      'no-redeclare': 'error', // suspicious/noRedeclare
      'no-regex-spaces': 'error', // complexity/noAdjacentSpacesInRegex
      'no-script-url': 'warn', // security/noScriptUrl
      'no-self-assign': 'error', // correctness/noSelfAssign
      'no-self-compare': 'error', // suspicious/noSelfCompare
      'no-sequences': 'error', // complexity/noCommaOperator
      'no-setter-return': 'error', // correctness/noSetterReturn
      'no-shadow-restricted-names': 'error', // suspicious/noShadowRestrictedNames
      'no-sparse-arrays': 'error', // suspicious/noSparseArray
      'no-template-curly-in-string': 'warn', // suspicious/noTemplateCurlyInString
      'no-this-before-super': 'error', // correctness/noUnreachableSuper
      'no-unneeded-ternary': 'error', // complexity/noUselessTernary
      'no-unreachable': 'error', // correctness/noUnreachable
      'no-unsafe-finally': 'error', // correctness/noUnsafeFinally
      'no-unsafe-negation': 'error', // suspicious/noUnsafeNegation
      'no-unsafe-optional-chaining': 'error', // correctness/noUnsafeOptionalChaining
      'no-unused-labels': 'error', // correctness/noUnusedLabels
      'no-unused-private-class-members': 'error', // correctness/noUnusedPrivateClassMembers
      'no-unused-vars': 'warn', // correctness/noUnusedFunctionParameters, correctness/noUnusedImports, correctness/noUnusedVariables
      'no-use-before-define': [
        'error',
        {
          functions: false,
          classes: false,
          variables: false,
          typedefs: false,
          ignoreTypeReferences: true,
        },
      ], // correctness/noInvalidUseBeforeDeclaration
      'no-useless-backreference': 'error', // suspicious/noUselessRegexBackrefs
      'no-useless-catch': 'error', // complexity/noUselessCatch
      'no-useless-constructor': 'error', // complexity/noUselessConstructor
      'no-useless-escape': 'error', // complexity/noUselessEscapeInRegex, suspicious/noUselessEscapeInString
      'no-useless-rename': 'error', // complexity/noUselessRename
      'no-with': 'error', // suspicious/noWith
      'oxc/no-accumulating-spread': 'error', // performance/noAccumulatingSpread
      'oxc/no-const-enum': 'error', // suspicious/noConstEnum
      'prefer-arrow-callback': 'warn', // complexity/useArrowFunction
      'prefer-const': 'error', // style/useConst
      'prefer-exponentiation-operator': 'error', // style/useExponentiationOperator
      'prefer-numeric-literals': 'error', // complexity/useNumericLiterals
      'prefer-regex-literals': 'error', // complexity/useRegexLiterals
      'prefer-rest-params': 'error', // complexity/noArguments
      'prefer-template': 'error', // style/useTemplate
      radix: 'error', // correctness/useParseIntRadix
      'react/button-has-type': 'error', // a11y/useButtonType
      'react/exhaustive-deps': 'warn', // correctness/useExhaustiveDependencies
      'react/jsx-key': 'error', // correctness/useJsxKeyInIterable
      'react/jsx-no-comment-textnodes': 'error', // suspicious/noCommentText
      'react/jsx-no-duplicate-props': 'error', // suspicious/noDuplicateJsxProps
      'react/jsx-no-target-blank': 'error', // security/noBlankTarget
      'react/jsx-no-useless-fragment': 'warn', // complexity/noUselessFragments
      'react/no-array-index-key': 'error', // suspicious/noArrayIndexKey
      'react/no-children-prop': 'error', // correctness/noChildrenProp
      'react/no-danger': 'error', // security/noDangerouslySetInnerHtml
      'react/no-danger-with-children': 'error', // security/noDangerouslySetInnerHtmlWithChildren
      'react/no-render-return-value': 'error', // correctness/noRenderReturnValue
      'react/no-unknown-property': 'error', // correctness/noUnknownProperty
      'react/rules-of-hooks': 'error', // correctness/useHookAtTopLevel
      'react/void-dom-elements-no-children': 'error', // correctness/noVoidElementsWithChildren
      'require-yield': 'error', // correctness/useYield
      'typescript/adjacent-overload-signatures': 'error', // suspicious/useAdjacentOverloadSignatures
      'typescript/ban-ts-comment': 'error', // suspicious/noTsIgnore
      'typescript/consistent-type-imports': 'warn', // style/useImportType
      'typescript/no-duplicate-enum-values': 'error', // correctness/noDuplicateEnumValueNames, suspicious/noDuplicateEnumValues
      'typescript/no-empty-interface': 'error', // suspicious/noEmptyInterface
      'typescript/no-explicit-any': 'warn', // suspicious/noExplicitAny
      'typescript/no-extra-non-null-assertion': 'error', // suspicious/noExtraNonNullAssertion
      'typescript/no-invalid-void-type': 'warn', // suspicious/noConfusingVoidType
      'typescript/no-misused-new': 'error', // suspicious/noMisleadingInstantiator
      'typescript/no-non-null-asserted-optional-chain': 'error', // suspicious/noNonNullAssertedOptionalChain
      'typescript/no-non-null-assertion': 'warn', // style/noNonNullAssertion
      'typescript/no-this-alias': 'error', // complexity/noUselessThisAlias
      'typescript/no-unnecessary-type-constraint': 'error', // complexity/noUselessTypeConstraint
      'typescript/no-unsafe-declaration-merging': 'error', // suspicious/noUnsafeDeclarationMerging
      'typescript/no-unsafe-function-type': 'error', // complexity/noBannedTypes
      'typescript/no-useless-empty-export': 'error', // complexity/noUselessEmptyExport
      'typescript/prefer-function-type': 'error', // style/useShorthandFunctionType
      'typescript/prefer-literal-enum-member': 'error', // style/useLiteralEnumMembers
      'typescript/prefer-namespace-keyword': 'error', // suspicious/useNamespaceKeyword
      'unicorn/no-document-cookie': 'error', // suspicious/noDocumentCookie
      'unicorn/no-instanceof-array': 'error', // suspicious/useIsArray
      'unicorn/no-static-only-class': 'error', // complexity/noStaticOnlyClass
      'unicorn/no-thenable': 'error', // suspicious/noThenProperty
      'unicorn/no-useless-switch-case': 'error', // complexity/noUselessSwitchCase
      'unicorn/prefer-array-flat-map': 'error', // complexity/useFlatMap
      'unicorn/prefer-array-index-of': 'error', // complexity/useIndexOf
      'unicorn/prefer-date-now': 'error', // complexity/useDateNow
      'unicorn/prefer-node-protocol': 'error', // style/useNodejsImportProtocol
      'unicorn/prefer-number-properties': 'warn', // suspicious/noGlobalIsFinite, suspicious/noGlobalIsNan
      'use-isnan': 'error', // correctness/useIsNan
      'valid-typeof': 'error', // correctness/useValidTypeof
    },
    // Biome's per-file overrides, carried over.
    overrides: [
      {
        files: [
          'packages/studio-ui/src/lib/observeResize.ts',
          'packages/studio-ui/src/lib/AnimatedHeight.tsx',
        ],
        rules: { 'no-cond-assign': 'off', 'react/exhaustive-deps': 'off' },
      },
      {
        files: [
          'packages/studio-ui/src/terminal/output.ts',
          'packages/studio-ui/src/composer/doc.ts',
          'packages/studio-ui/src/composer/richText.ts',
          'packages/studio-ui/src/composer/listContinuation.ts',
          'packages/studio-ui/src/composer/codeBlock.ts',
          'packages/studio-ui/src/composer/undoGrouping.ts',
          'packages/studio-ui/src/composer/contextReferences.ts',
          'packages/studio-ui/src/composer/shared/composerTrigger.ts',
          'packages/studio-ui/src/composer/shared/composerInlineTokens.ts',
          'packages/studio-ui/src/composer/shared/composerContextReferences.ts',
          'packages/studio-ui/src/composer/shared/inlineSkills.ts',
          'packages/studio-ui/src/composer/shared/searchRanking.ts',
        ],
        rules: { 'typescript/no-non-null-assertion': 'off' },
      },
      {
        files: ['packages/studio-ui/src/chat/tools/commandLabel.ts'],
        rules: { 'no-control-regex': 'off', 'typescript/no-non-null-assertion': 'off' },
      },
    ],
  },
  fmt: {
    // Explicit: Oxfmt reads the root .editorconfig (indent_size = 4), which Biome ignored.
    useTabs: false,
    tabWidth: 2,
    printWidth: 100,
    singleQuote: true,
    // Keep package.json key order as written; sorting would rewrite every manifest.
    sortPackageJson: false,
    // Markdown stays as written: Biome never formatted it, and Oxfmt pads tables.
    ignorePatterns: ['**/dist/**', '**/*.md', ...generated],
    // Biome's two double-quote overrides: Studio's code vendored from T3 Code keeps its style.
    overrides: [
      {
        files: ['packages/studio-ui/src/**', 'packages/ui/src/sidebar-state.ts'],
        options: { singleQuote: false },
      },
    ],
  },
});
