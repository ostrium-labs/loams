// The conformance report: what the suite writes, and where.
//
// `sdks/conformance/required.mjs` is the authority. A suite writes
// `sdks/fixtures/results/<language>.json`:
//
//     { "tests": [...], "ran": [...], "skipped": [{ "fixture": "…", "reason": "…" }] }
//
// and `ran` is the only thing that counts: "a test that passes without touching a
// required fixture has not run it." So the report here is written from what the
// corpus driver **executed and checked**, never from a list, and a fixture that
// could not be reached is absent from `ran` rather than skipped — because
// `maySkip()` in `required.mjs` permits exactly one skip, a fixture marked
// `transport: grpc-only` on the Connect-unary fallback transport, and this SDK is
// not on that fallback: it speaks Connect, gRPC and gRPC-Web over HTTP, so no
// fixture is `grpc-only` from its point of view and `skipped` is empty by
// construction.

using System.Text.Json;
using System.Text.Json.Nodes;

namespace Loams.Tests.Support;

/// <summary>
/// Writes <c>sdks/fixtures/results/csharp.json</c> from a run.
/// </summary>
/// <remarks>
/// The shape is the one `required.mjs` reads, field for field, and the values are
/// the run's own:
/// </remarks>
/// <param name="repositoryRoot">The repository root, which is two directories above this project.</param>
public static class ConformanceReport
{
    /// <summary>
    /// The six canonical test names, in the order `REQUIRED_TESTS` states them.
    /// </summary>
    /// <remarks>
    /// These literals are the contract: `sdks/conformance/check-languages.mjs` greps
    /// this suite's **source** for them, and
    /// <c>csharp_conformance_test_names_exist</c> asserts the set cannot shrink. They
    /// are written out rather than derived from the method names so a rename is a
    /// visible edit here rather than a silent change in a grep's result.
    /// </remarks>
    public static readonly string[] RequiredTestNames =
    [
        "csharp_conformance_all_required_fixtures",
        "csharp_retry_reuses_idempotency_key",
        "csharp_error_reason_mapping",
        "csharp_stream_resume_with_cursor",
        "csharp_token_source_refresh",
        "csharp_pagination_iterator",
    ];

    /// <summary>Where the report goes, relative to the repository root.</summary>
    public const string ReportPath = "sdks/fixtures/results/csharp.json";

    /// <summary>
    /// Writes the report, and returns where it wrote it.
    /// </summary>
    /// <remarks>
    /// `transport` is <c>connect</c> rather than <c>connect-unary</c>: the SDK is a
    /// Connect client with gRPC and gRPC-Web also available, and
    /// <c>maySkip()</c>'s one permitted skip is conditioned on the Connect-unary
    /// **fallback** transport that D613 gives Ruby and PHP. This SDK is not a
    /// fallback, so a fixture marked <c>grpc-only</c> would be a real gap rather than
    /// a permitted skip — and recording the transport honestly is what makes the
    /// gate say so.
    /// </remarks>
    public static string Write(string repositoryRoot, CorpusRun run, IReadOnlyList<string> tests)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(repositoryRoot);
        ArgumentNullException.ThrowIfNull(run);

        var report = new JsonObject
        {
            ["about"] =
                "What this SDK's suite ran. `ran` is what `CorpusDriver` replayed through the SDK's " +
                "public surface and checked against each recording's own `expect` block; a fixture " +
                "that could not be reached is absent from it rather than skipped, because " +
                "sdks/conformance/required.mjs permits exactly one skip (a `transport: grpc-only` " +
                "fixture on the Connect-unary fallback) and this SDK is not that fallback.",
            ["language"] = "csharp",
            ["transport"] = "connect",
            ["tests"] = new JsonArray([.. tests.Select((name) => (JsonNode)name!)]),
            ["ran"] = new JsonArray([.. run.Ran.Select((name) => (JsonNode)name!)]),
            ["skipped"] = new JsonArray(),
        };

        var path = Path.Combine(repositoryRoot, ReportPath);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, report.ToJsonString(new JsonSerializerOptions { WriteIndented = true }) + "\n");
        return path;
    }

    /// <summary>
    /// The repository root, found by walking up from this assembly until
    /// <c>sdks/fixtures/manifest.json</c> appears.
    /// </summary>
    /// <remarks>
    /// Walking up rather than counting `../../..`: the test assembly lives under
    /// <c>bin/Debug/net8.0</c> by default and under <c>bin/Release/net8.0</c> when it
    /// is published, so a counted path is right only for the configuration somebody
    /// happened to run. The manifest is the thing that identifies the root, and it
    /// exists in exactly one place.
    /// </remarks>
    public static string RepositoryRoot()
    {
        var directory = new DirectoryInfo(AppContext.BaseDirectory);
        while (directory is not null)
        {
            if (File.Exists(Path.Combine(directory.FullName, "sdks", "fixtures", "manifest.json")))
            {
                return directory.FullName;
            }
            directory = directory.Parent;
        }
        throw new InvalidOperationException(
            $"no sdks/fixtures/manifest.json above {AppContext.BaseDirectory}, so this is not a " +
            "checkout of the repository and the conformance corpus cannot be found");
    }
}