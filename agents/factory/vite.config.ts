import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite-plus';

export default defineConfig({
  resolve: {
    // The Live app's bundle (`live/factory.js`) runs in tests on a fake `loams:server`.
    alias: [
      {
        find: /^loams:server$/,
        replacement: fileURLToPath(new URL('./test/support/loams-server.ts', import.meta.url)),
      },
    ],
  },
  test: { include: ['test/**/*.test.ts'] },
  // Lint and format (design 79 D1773, plan VV1 item 3): Oxlint and Oxfmt through
  // `vp check`, replacing Biome. The rules are VV0 Task 1's parity table, the same
  // set as web/vite.config.ts without the React and a11y rules.
  lint: {
    plugins: ['typescript', 'unicorn', 'oxc'],
    categories: { correctness: 'off' },
    ignorePatterns: ['**/dist/**'],
    rules: {
      'array-callback-return': 'error', // suspicious/useIterableCallbackReturn
      'constructor-super': 'error', // correctness/noInvalidConstructorSuper
      'default-case-last': 'error', // suspicious/useDefaultSwitchClauseLast
      eqeqeq: 'error', // suspicious/noDoubleEquals
      'for-direction': 'error', // correctness/useValidForDirection
      'getter-return': 'error', // suspicious/useGetterReturn
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
      'no-fallthrough': 'error', // suspicious/noFallthroughSwitchClause
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
      'no-script-url': 'error', // security/noScriptUrl
      'no-self-assign': 'error', // correctness/noSelfAssign
      'no-self-compare': 'error', // suspicious/noSelfCompare
      'no-sequences': 'error', // complexity/noCommaOperator
      'no-setter-return': 'error', // correctness/noSetterReturn
      'no-shadow-restricted-names': 'error', // suspicious/noShadowRestrictedNames
      'no-sparse-arrays': 'error', // suspicious/noSparseArray
      'no-template-curly-in-string': 'error', // suspicious/noTemplateCurlyInString
      'no-this-before-super': 'error', // correctness/noUnreachableSuper
      'no-unneeded-ternary': 'error', // complexity/noUselessTernary
      'no-unreachable': 'error', // correctness/noUnreachable
      'no-unsafe-finally': 'error', // correctness/noUnsafeFinally
      'no-unsafe-negation': 'error', // suspicious/noUnsafeNegation
      'no-unsafe-optional-chaining': 'error', // correctness/noUnsafeOptionalChaining
      'no-unused-labels': 'error', // correctness/noUnusedLabels
      'no-unused-private-class-members': 'error', // correctness/noUnusedPrivateClassMembers
      'no-unused-vars': 'error', // correctness/noUnusedFunctionParameters, correctness/noUnusedImports, correctness/noUnusedVariables
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
      'prefer-arrow-callback': 'error', // complexity/useArrowFunction
      'prefer-const': 'error', // style/useConst
      'prefer-exponentiation-operator': 'error', // style/useExponentiationOperator
      'prefer-numeric-literals': 'error', // complexity/useNumericLiterals
      'prefer-regex-literals': 'error', // complexity/useRegexLiterals
      'prefer-rest-params': 'error', // complexity/noArguments
      'prefer-template': 'error', // style/useTemplate
      radix: 'error', // correctness/useParseIntRadix
      'require-yield': 'error', // correctness/useYield
      'typescript/adjacent-overload-signatures': 'error', // suspicious/useAdjacentOverloadSignatures
      'typescript/ban-ts-comment': 'error', // suspicious/noTsIgnore
      'typescript/consistent-type-imports': 'error', // style/useImportType
      'typescript/no-duplicate-enum-values': 'error', // correctness/noDuplicateEnumValueNames, suspicious/noDuplicateEnumValues
      'typescript/no-empty-interface': 'error', // suspicious/noEmptyInterface
      'typescript/no-explicit-any': 'error', // suspicious/noExplicitAny
      'typescript/no-extra-non-null-assertion': 'error', // suspicious/noExtraNonNullAssertion
      'typescript/no-invalid-void-type': 'error', // suspicious/noConfusingVoidType
      'typescript/no-misused-new': 'error', // suspicious/noMisleadingInstantiator
      'typescript/no-non-null-asserted-optional-chain': 'error', // suspicious/noNonNullAssertedOptionalChain
      'typescript/no-non-null-assertion': 'error', // style/noNonNullAssertion
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
      'unicorn/prefer-number-properties': 'error', // suspicious/noGlobalIsFinite, suspicious/noGlobalIsNan
      'use-isnan': 'error', // correctness/useIsNan
      'valid-typeof': 'error', // correctness/useValidTypeof
    },
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
    ignorePatterns: ['**/dist/**', '**/*.md'],
  },
});
