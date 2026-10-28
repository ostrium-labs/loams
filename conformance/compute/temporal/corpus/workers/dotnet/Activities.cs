// The corpus's .NET activities (TSDK-dotnet), named as the Go and TypeScript
// ones so the histories carry the same activity types.
using Temporalio.Activities;
using Temporalio.Exceptions;

namespace Loams.Corpus;

public class CorpusActivities
{
    [Activity("greet")]
    public Task<string> Greet(string name) => Task.FromResult($"hello {name}");

    [Activity("heartbeat_thrice")]
    public Task<int> HeartbeatThrice()
    {
        for (var i = 1; i <= 3; i++)
        {
            ActivityExecutionContext.Current.Heartbeat(i);
        }
        return Task.FromResult(3);
    }

    [Activity("fail_twice")]
    public Task<int> FailTwice()
    {
        var attempt = ActivityExecutionContext.Current.Info.Attempt;
        if (attempt < 3)
        {
            throw new ApplicationFailureException($"attempt {attempt} fails", "Transient");
        }
        return Task.FromResult(attempt);
    }

    [Activity("sleep_for")]
    public async Task SleepFor(int ms)
    {
        // Cancelled when the worker learns that the server timed the attempt
        // out; the scenarios only look at the workflow side.
        await Task.Delay(ms, ActivityExecutionContext.Current.CancellationToken);
    }

    [Activity("large")]
    public Task<string> Large(int n) => Task.FromResult(new string('x', n));
}
