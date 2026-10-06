# Loams SDKs

This directory contains official language SDKs, code generation templates, and conformance test suites for Loams.

## Supported Languages

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

## Conformance & Testing

- **[`conformance/`](conformance/)**: Cross-language conformance test runner verifying that all SDKs adhere to the Loams runtime contract (idempotency, consistency tokens, error representations, and pagination).
- **[`fixtures/`](fixtures/)**: Golden test vectors and fixtures consumed by all SDK conformance test suites.
- **[`templates/`](templates/)**: Generator templates used by `scripts/sdk/gen.sh` to produce unified facades and typed clients.
