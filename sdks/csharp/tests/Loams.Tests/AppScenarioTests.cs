// SDKP Parts B and C (P0d): the app scenario through the public facade.
//
// csharp_app_collections, csharp_app_search, csharp_app_graph, and (Part C)
// csharp_app_sql and csharp_app_streams replay sdks/fixtures/recorded/app/ against
// the shared fixture server (sdks/fixtures/app-scenario.md says how; the replay is
// Support/AppReplay.cs). Each fixture that replays clean is added to the report's
// `ran`, which `check-languages --check csharp --suite app` reads; the test names
// are found by a static scan of this file, so they are spelled out.
//
// The shared server needs Node. Without it a test passes without replaying, and the
// fixture is absent from `ran`, so the app-suite gate (not this suite) says so.

using Loams.Tests.Support;
using Xunit;

namespace Loams.Tests;

public sealed class AppScenarioTests
{
    [Fact]
    public Task csharp_app_collections() => Replay("app_collections");

    [Fact]
    public Task csharp_app_search() => Replay("app_search");

    [Fact]
    public Task csharp_app_graph() => Replay("app_graph");

    [Fact]
    public Task csharp_app_sql() => Replay("app_sql");

    [Fact]
    public Task csharp_app_streams() => Replay("app_streams");

    private static async Task Replay(string fixture)
    {
        Assert.True(AppReplay.Available(fixture), $"no recording sdks/fixtures/recorded/app/{fixture}.json");
        var root = ConformanceReport.RepositoryRoot();
        using var server = FixtureServer.StartShared(Path.Combine(root, "sdks", "conformance"));
        if (server is null)
        {
            Console.Error.WriteLine($"{fixture}: the shared fixture server needs node; not replayed");
            return;
        }

        var failures = await AppReplay.RunAsync(server.Endpoint, fixture);
        Assert.True(failures.Count == 0,
            $"{failures.Count} failure(s) replaying {fixture}:\n  - " + string.Join("\n  - ", failures));
        AppReplays.Record(fixture);
    }
}
