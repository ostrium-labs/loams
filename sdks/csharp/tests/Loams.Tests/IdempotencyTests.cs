// R3's idempotency-key lifecycle, pinned as unit tests.
//
// The corpus is what settled the `mint` argument below. Every idempotency key the
// recorded fixtures carry is a literal the caller wrote -- `conformance-idempotency-1`,
// `conformance-stream-resume-1` -- and every mutating fixture that has no key was
// recorded without one. `fixture-server.mjs` compares the request byte for byte, so
// minting a key on a replayed request fails the fixture on the request itself, before
// the refusal it exists to test is ever read.
//
// So minting is the **default** and stays the default, because R3's whole point is
// that a caller who never thought about keys still gets retry safety. Turning it off
// is a per-call choice, for the two callers that need it: replaying a recorded
// request, and a caller whose own storage owns the key.

using Loams;
using Xunit;

namespace Loams.Tests;

/// <summary>R3's idempotency-key lifecycle.</summary>
public sealed class IdempotencyTests
{
    /// <summary>
    /// By default the SDK still mints an idempotency key (R3).
    /// </summary>
    /// <remarks>
    /// The opt-out below must not become the default. R3 says a mutating call is
    /// given a key per logical call, and a caller who loses that by default loses
    /// retry safety on every call they did not think about.
    /// </remarks>
    [Fact]
    public void csharp_idempotency_mints_a_key_by_default()
    {
        var request = new Loams.Approvals.V1.DecideApprovalRequest { ApprovalId = "apr_01" };

        var keyed = Idempotency.Apply(request, supplied: string.Empty, mint: true);

        Assert.True(keyed.Keyed);
        Assert.NotEqual(string.Empty, ReadKey(keyed.Request));
    }

    /// <summary>
    /// A caller's own key wins over a minted one, on both paths.
    /// </summary>
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public void csharp_idempotency_keeps_a_caller_supplied_key(bool mint)
    {
        var request = new Loams.Approvals.V1.DecideApprovalRequest
        {
            ApprovalId = "apr_01",
            IdempotencyKey = "conformance-idempotency-1",
        };

        var keyed = Idempotency.Apply(request, supplied: string.Empty, mint);

        Assert.True(keyed.Keyed);
        Assert.Equal("conformance-idempotency-1", ReadKey(keyed.Request));
    }

    /// <summary>
    /// With minting off, a request that carries no key goes out exactly as built.
    /// </summary>
    /// <remarks>
    /// This is what replays a recorded request. The corpus's
    /// <c>mock_error_approval_expired</c> was recorded with
    /// <c>{"approvalId":…,"revision":"2","decision":…}</c> and no key, and
    /// <c>fixture-server.mjs</c> compares the request byte for byte — so an SDK that
    /// mints a key fails every mutating fixture in the corpus on the request, before
    /// the error mapping it was meant to test ever runs. Every key the corpus does
    /// carry (<c>conformance-idempotency-1</c>, <c>conformance-stream-resume-1</c>)
    /// is a literal the caller wrote, which is what makes "the caller supplies it"
    /// the rule the corpus states rather than the rule it contradicts.
    /// </remarks>
    [Fact]
    public void csharp_idempotency_sends_the_request_as_built_when_not_minting()
    {
        var request = new Loams.Approvals.V1.DecideApprovalRequest
        {
            ApprovalId = "apr_01J9EXPIRED",
            Revision = 2,
            Decision = Loams.Approvals.V1.DecisionKind.Approve,
        };

        var keyed = Idempotency.Apply(request, supplied: string.Empty, mint: false);

        Assert.False(keyed.Keyed);
        Assert.Same(request, keyed.Request);
        Assert.Equal(
            """{"approvalId":"apr_01J9EXPIRED","revision":"2","decision":"DECISION_KIND_APPROVE"}""",
            CompactJson.Format(keyed.Request));
    }

    /// <summary>
    /// A message whose schema declares no key is left alone on both paths.
    /// </summary>
    /// <remarks>
    /// Keying it would invent a field the schema does not declare, and the byte
    /// comparison in the corpus would reject the request.
    /// </remarks>
    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public void csharp_idempotency_leaves_a_message_without_the_field_alone(bool mint)
    {
        var request = new Loams.Instance.V1.GetInstanceRequest();

        var keyed = Idempotency.Apply(request, supplied: string.Empty, mint);

        Assert.False(keyed.Keyed);
        Assert.Same(request, keyed.Request);
    }

    private static string ReadKey(Google.Protobuf.IMessage message) =>
        message.Descriptor.FindFieldByName("idempotency_key")!.Accessor.GetValue(message) as string ?? string.Empty;
}
