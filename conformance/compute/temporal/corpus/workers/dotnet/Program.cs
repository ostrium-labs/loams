// The .NET half of the corpus (TSDK-dotnet): a worker for Workflows.cs, a
// starter (driver) per scenario with the fixed workflow ID
// corpus-dotnet-<scenario>, and the replayer. The Go exporter writes the
// histories afterwards (`corpus-go export -prefix corpus-dotnet-`), so every
// language's files come from one exporter.
//
//   corpus-dotnet generate <address> <namespace> <out.json> [scenario,...] [--no-search-attribute]
//   corpus-dotnet replay <histories-dir>
//
// TEMPORAL_API_KEY: a bearer token on every call, in plain text (the Loams
// gateway's API keys; its compose profile has no TLS). Unset, nothing changes.
using System.Text.Json;
using Google.Protobuf.WellKnownTypes;
using Loams.Corpus;
using Temporalio.Api.Enums.V1;
using Temporalio.Api.OperatorService.V1;
using Temporalio.Api.WorkflowService.V1;
using Temporalio.Client;
using Temporalio.Common;
using Temporalio.Exceptions;
using Temporalio.Worker;

var rest = args.Skip(1).ToArray();
try
{
    return args.FirstOrDefault() switch
    {
        "generate" => await Generator.RunAsync(rest),
        "replay" => await Replayer.RunAsync(rest),
        _ => Usage(),
    };
}
catch (Exception e)
{
    Console.Error.WriteLine($"corpus-dotnet: {e}");
    return 1;
}

static int Usage()
{
    Console.Error.WriteLine("usage: corpus-dotnet generate <address> <namespace> <out.json> [scenarios] [--no-search-attribute]");
    Console.Error.WriteLine("       corpus-dotnet replay <histories-dir>");
    return 2;
}

namespace Loams.Corpus
{
    internal sealed class Observation
    {
        public Observation(string scenario, string workflowId)
        {
            Scenario = scenario;
            WorkflowId = workflowId;
        }

        public string Scenario { get; }

        public string WorkflowId { get; }

        public string? Result { get; set; }

        public List<string> Expected { get; } = new();

        public Dictionary<string, object?> ToJson()
        {
            var d = new Dictionary<string, object?> { ["scenario"] = Scenario, ["workflow_id"] = WorkflowId };
            if (Result != null)
            {
                d["result"] = Result;
            }
            if (Expected.Count > 0)
            {
                d["expected"] = Expected;
            }
            return d;
        }
    }

    internal static class Generator
    {
        public const string Queue = "corpus-dotnet";
        public const string Prefix = "corpus-dotnet-";

        public static async Task<int> RunAsync(string[] argv)
        {
            var noSearchAttribute = argv.Contains("--no-search-attribute");
            var pos = argv.Where(a => !a.StartsWith("--", StringComparison.Ordinal)).ToArray();
            if (pos.Length < 3)
            {
                throw new ArgumentException("generate needs <address> <namespace> <out.json>");
            }
            var (address, ns, outPath) = (pos[0], pos[1], pos[2]);
            var want = pos.Length > 3 && pos[3].Length > 0 ? new HashSet<string>(pos[3].Split(',')) : null;

            var client = await TemporalClient.ConnectAsync(ConnectOptions(address, ns));
            await EnsureNamespaceAsync(client, ns, !noSearchAttribute);

            var options = new TemporalWorkerOptions(Queue)
            {
                // One activity at a time, so the parallel fan-out's activities
                // complete in the order they were scheduled (as the Go worker).
                MaxConcurrentActivities = 1,
            };
            foreach (var t in Scenarios.Workflows)
            {
                options.AddWorkflow(t);
            }
            options.AddAllActivities(new CorpusActivities());

            var observations = new List<Observation>();
            using var worker = new TemporalWorker(client, options);
            await worker.ExecuteAsync(async () =>
            {
                foreach (var (name, _) in Scenarios.All)
                {
                    if (want != null && !want.Contains(name))
                    {
                        continue;
                    }
                    var o = await DriveAsync(client, name, Prefix + name);
                    Console.WriteLine($"corpus: {name}: {o.Result ?? string.Empty}");
                    observations.Add(o);
                }
            });

            var doc = new Dictionary<string, object?>
            {
                ["namespace"] = ns,
                ["language"] = "dotnet",
                ["scenarios"] = observations.Select(o => o.ToJson()).ToList(),
            };
            await File.WriteAllTextAsync(
                outPath,
                JsonSerializer.Serialize(doc, new JsonSerializerOptions { WriteIndented = true }) + "\n");
            return 0;
        }

        public static TemporalClientConnectOptions ConnectOptions(string address, string ns)
        {
            var o = new TemporalClientConnectOptions(address) { Namespace = ns };
            var key = Environment.GetEnvironmentVariable("TEMPORAL_API_KEY");
            if (!string.IsNullOrEmpty(key))
            {
                // Not `ApiKey`: the header only, in plain text, as the Go and
                // TypeScript drivers send it.
                o.RpcMetadata = new Dictionary<string, string> { ["authorization"] = $"Bearer {key}" };
            }
            return o;
        }

        /// <summary>Registers the namespace (72 h retention, DX1 R0.9) and the
        /// corpus's custom search attribute, both idempotently (as the Go
        /// generator's ensureNamespace).</summary>
        private static async Task EnsureNamespaceAsync(ITemporalClient client, string ns, bool searchAttribute)
        {
            try
            {
                await client.WorkflowService.RegisterNamespaceAsync(new RegisterNamespaceRequest
                {
                    Namespace = ns,
                    WorkflowExecutionRetentionPeriod = Duration.FromTimeSpan(TimeSpan.FromHours(72)),
                });
            }
            catch (RpcException e) when (e.Code == RpcException.StatusCode.AlreadyExists)
            {
            }
            if (!searchAttribute)
            {
                return;
            }
            // A new namespace takes a moment to reach every service's cache.
            var deadline = DateTime.UtcNow.AddMinutes(1);
            while (true)
            {
                try
                {
                    var req = new AddSearchAttributesRequest { Namespace = ns };
                    req.SearchAttributes.Add(Opts.SearchAttribute, IndexedValueType.Keyword);
                    await client.OperatorService.AddSearchAttributesAsync(req);
                    break;
                }
                catch (RpcException e) when (e.Code == RpcException.StatusCode.AlreadyExists)
                {
                    break;
                }
                catch (RpcException) when (DateTime.UtcNow < deadline)
                {
                    await Task.Delay(TimeSpan.FromSeconds(2));
                }
            }
            await Task.Delay(TimeSpan.FromSeconds(3));
        }

        /// <summary>Waits until the run's first workflow task completed, so a
        /// signal, query, cancel or update lands on a running workflow.</summary>
        private static async Task WaitStartedAsync(WorkflowHandle h)
        {
            while (true)
            {
                var d = await h.DescribeAsync();
                if (d.HistoryLength >= 4 && d.RawDescription.PendingWorkflowTask == null)
                {
                    return;
                }
                await Task.Delay(200);
            }
        }

        private static async Task SettleAsync(Observation o, WorkflowHandle h)
        {
            var result = await h.GetResultAsync<JsonElement?>();
            o.Result = result is { } r ? r.GetRawText() : "null";
        }

        private static async Task<Observation> DriveAsync(ITemporalClient client, string name, string id)
        {
            var o = new Observation(name, id);
            WorkflowOptions Options() => new(id, Queue);
            Task<WorkflowHandle> Start(object?[]? args = null, WorkflowOptions? options = null) =>
                client.StartWorkflowAsync(name, args ?? Array.Empty<object?>(), options ?? Options());

            switch (name)
            {
                case "hello":
                    await SettleAsync(o, await Start(new object?[] { "corpus" }));
                    return o;

                case "signal_then_complete":
                {
                    var h = await Start();
                    await WaitStartedAsync(h);
                    await h.SignalAsync("go", new object?[] { "s" });
                    await SettleAsync(o, h);
                    return o;
                }

                case "signal_with_start":
                {
                    var opts = Options();
                    opts.SignalWithStart("go", new object?[] { "s" });
                    await SettleAsync(o, await Start(options: opts));
                    return o;
                }

                case "query_while_running":
                {
                    var h = await Start();
                    await WaitStartedAsync(h);
                    var state = await h.QueryAsync<string>("state", Array.Empty<object?>());
                    if (state != "waiting")
                    {
                        throw new InvalidOperationException($"query answered {state}");
                    }
                    o.Expected.Add("query state=waiting");
                    await h.SignalAsync("finish", Array.Empty<object?>());
                    await SettleAsync(o, h);
                    return o;
                }

                case "cancel_requested":
                case "terminated":
                {
                    var h = await Start();
                    await WaitStartedAsync(h);
                    if (name == "cancel_requested")
                    {
                        await h.CancelAsync();
                    }
                    else
                    {
                        await h.TerminateAsync("corpus terminates it");
                    }
                    try
                    {
                        await h.GetResultAsync();
                    }
                    catch (WorkflowFailedException e)
                    {
                        o.Expected.Add($"{e.InnerException?.GetType().Name}: {e.InnerException?.Message}");
                        return o;
                    }
                    throw new InvalidOperationException($"the workflow completed instead of closing as {name}");
                }

                case "workflow_failed_application_error":
                {
                    var h = await Start();
                    try
                    {
                        await h.GetResultAsync();
                    }
                    catch (WorkflowFailedException e) when (e.InnerException is ApplicationFailureException af)
                    {
                        o.Expected.Add($"{af.ErrorType}: {af.Message}");
                        return o;
                    }
                    throw new InvalidOperationException("want an application error");
                }

                case "continue_as_new_3":
                    await SettleAsync(o, await Start(new object?[] { 0 }));
                    return o;

                case "id_reuse_allow_duplicate_failed_only":
                {
                    WorkflowOptions Policy()
                    {
                        var p = Options();
                        p.IdReusePolicy = WorkflowIdReusePolicy.AllowDuplicateFailedOnly;
                        return p;
                    }
                    var first = await Start(new object?[] { true }, Policy());
                    try
                    {
                        await first.GetResultAsync();
                        throw new InvalidOperationException("the first run should fail");
                    }
                    catch (WorkflowFailedException)
                    {
                    }
                    var second = await Start(new object?[] { false }, Policy());
                    o.Expected.Add("the first run failed; the second started with the same ID");
                    await SettleAsync(o, second);
                    return o;
                }

                case "id_conflict_fail":
                {
                    WorkflowOptions Policy()
                    {
                        var p = Options();
                        p.IdConflictPolicy = WorkflowIdConflictPolicy.Fail;
                        return p;
                    }
                    var h = await Start(options: Policy());
                    try
                    {
                        await Start(options: Policy());
                        throw new InvalidOperationException("the second start should be refused");
                    }
                    catch (WorkflowAlreadyStartedException)
                    {
                        o.Expected.Add("the second start was refused: WorkflowExecutionAlreadyStarted");
                    }
                    await h.SignalAsync("finish", Array.Empty<object?>());
                    await SettleAsync(o, h);
                    return o;
                }

                case "memo":
                {
                    var opts = Options();
                    opts.Memo = new Dictionary<string, object> { ["initial"] = "set at start" };
                    await SettleAsync(o, await Start(options: opts));
                    return o;
                }

                case "update_accepted_completed":
                case "update_rejected":
                {
                    var h = await Start();
                    await WaitStartedAsync(h);
                    var by = name == "update_rejected" ? -1 : 2;
                    try
                    {
                        var total = await h.ExecuteUpdateAsync<int>(
                            "increment", new object?[] { by }, new WorkflowUpdateOptions { Id = $"{id}-update" });
                        o.Expected.Add($"update returned {total}");
                        if (by < 0)
                        {
                            throw new InvalidOperationException("the validator should reject a negative increment");
                        }
                    }
                    catch (WorkflowUpdateFailedException e) when (by < 0)
                    {
                        o.Expected.Add($"update rejected: {e.InnerException?.Message ?? e.Message}");
                    }
                    await h.SignalAsync("finish", Array.Empty<object?>());
                    await SettleAsync(o, h);
                    return o;
                }

                default:
                    await SettleAsync(o, await Start());
                    return o;
            }
        }
    }

    /// <summary>The .NET SDK's own replayer over a directory of protojson
    /// histories (corpus_replays_dotnet; the gateway leg's
    /// gateway_histories_replay_dotnet): every history must replay with zero
    /// nondeterminism errors.</summary>
    internal static class Replayer
    {
        public static async Task<int> RunAsync(string[] argv)
        {
            if (argv.Length < 1)
            {
                throw new ArgumentException("replay needs <histories-dir>");
            }
            var dir = argv[0];
            var files = Directory.GetFiles(dir, "*.json").OrderBy(f => f, StringComparer.Ordinal).ToList();
            if (files.Count == 0)
            {
                Console.Error.WriteLine($"replay: no histories in {dir}");
                return 1;
            }
            var options = new WorkflowReplayerOptions();
            foreach (var t in Scenarios.Workflows)
            {
                options.AddWorkflow(t);
            }
            var replayer = new WorkflowReplayer(options);
            var failures = 0;
            foreach (var f in files)
            {
                // The workflow ID the generator used: child IDs derive from it.
                var scenario = ScenarioOf(Path.GetFileName(f));
                var history = WorkflowHistory.FromJson(Generator.Prefix + scenario, await File.ReadAllTextAsync(f));
                try
                {
                    await replayer.ReplayWorkflowAsync(history);
                    Console.WriteLine($"ok   {Path.GetFileName(f)}");
                }
                catch (Exception e)
                {
                    failures++;
                    Console.WriteLine($"FAIL {Path.GetFileName(f)}: {e.GetType().Name}: {e.Message}");
                }
            }
            Console.WriteLine($"replay: {files.Count - failures}/{files.Count} histories replay");
            return failures == 0 ? 0 : 1;
        }

        /// <summary>`hello.json` and `continue_as_new_3.run2.json` name their scenario.</summary>
        public static string ScenarioOf(string file)
        {
            var stem = file.EndsWith(".json", StringComparison.Ordinal) ? file[..^5] : file;
            var dot = stem.IndexOf(".run", StringComparison.Ordinal);
            return dot >= 0 ? stem[..dot] : stem;
        }
    }
}
