![Loams — Your data. Your bucket.](../docs/assets/loams-banner.svg)

# Loams SDKs

This directory contains Loams language implementations, code generation templates, and conformance fixtures. Packages are pre-release and not a stable published SDK family. Check each language’s README and the conformance runner before relying on a runtime or transport.

## Language implementations

| Language | Path | Key Packages / Modules |
| --- | --- | --- |
| **TypeScript** | [`typescript/`](typescript/) | `@loams/client`, `@loams/live` |
| **Python** | [`python/`](python/) | `loams` |
| **Rust** | [`rust/`](rust/) | `loams-sdk` |
| **Go** | [`go/`](go/) | `github.com/ostrium-labs/loams/sdks/go` |
| **C# (.NET)** | [`csharp/`](csharp/) | `Loams` |
| **Java** | [`java/`](java/) | `dev.loams` |
| **Kotlin** | [`kotlin/`](kotlin/) | `dev.loams.kotlin` |
| **Swift** | [`swift/`](swift/) | `Loams` |
| **Ruby** | [`ruby/`](ruby/) | `loams` |
| **C++** | [`cpp/`](cpp/) | `loams` |
| **Dart** | [`dart/`](dart/) | Implementation and runtime fixtures |
| **Objective-C** | [`objc/`](objc/) | Implementation and runtime fixtures |
| **PHP** | [`php/`](php/) | Implementation and runtime fixtures |

## Conformance & Testing

- **[`conformance/`](conformance/)**: Cross-language conformance test runner verifying that all SDKs adhere to the Loams runtime contract (idempotency, consistency tokens, error representations, and pagination).
- **[`fixtures/`](fixtures/)**: Golden test vectors and fixtures consumed by all SDK conformance test suites.
- **[`templates/`](templates/)**: Generator templates used by `scripts/sdk/gen.sh` to produce unified facades and typed clients.
