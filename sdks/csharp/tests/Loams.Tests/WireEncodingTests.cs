// Three encoding defects the recorded corpus found, pinned as unit tests so the
// byte-level mistakes they describe cannot come back through a different fixture.
//
// Each of these was found by replaying `sdks/fixtures` and reading the 400 the
// fixture server answers with, which names the expected and the sent bytes. None
// of them shows up in a round trip: a decode followed by an encode that drops a
// field still round trips, it just does not round trip **to the same bytes**, and
// the corpus compares bytes.
//
//   - an enum field that was set goes missing, because the generated CLR enum is
//     not an `EnumValueDescriptor` and the "is this the default?" test read it as
//     one, saw `null`, and concluded "unset";
//   - a detail's `value` arrives as base64 with its `=` padding stripped, which
//     `Convert.FromBase64String` rejects, and the rejection was swallowed into
//     "this error has no reason";
//   - a mutating call whose request carries no key gets one minted, which changes
//     the bytes of a replayed request that must match the recording exactly.

using Loams;
using Xunit;

namespace Loams.Tests;

/// <summary>The byte-level encoding rules the recorded corpus pins.</summary>
public sealed class WireEncodingTests
{
    /// <summary>
    /// A set enum field is written **by name**.
    /// </summary>
    /// <remarks>
    /// The recorded `DecideApproval` request is
    /// <c>{"approvalId":…,"revision":"1","decision":"DECISION_KIND_APPROVE"}</c>. A
    /// writer that drops <c>decision</c> because the CLR value is a generated enum
    /// rather than an <c>EnumValueDescriptor</c> sends a request that asks for a
    /// decision the caller never made — the server reads
    /// <c>DECISION_KIND_UNSPECIFIED</c> and refuses it. Silence here is a wrong
    /// request, not a missing field.
    /// </remarks>
    [Fact]
    public void csharp_json_writes_a_set_enum_by_name()
    {
        var request = new Loams.Approvals.V1.DecideApprovalRequest
        {
            ApprovalId = "apr_01J9CREATED",
            Revision = 1,
            Decision = Loams.Approvals.V1.DecisionKind.Approve,
        };

        Assert.Equal(
            """{"approvalId":"apr_01J9CREATED","revision":"1","decision":"DECISION_KIND_APPROVE"}""",
            CompactJson.Format(request));
    }

    /// <summary>
    /// An enum left at its zero value is omitted, which is what proto3 JSON says.
    /// </summary>
    /// <remarks>
    /// The other half of the rule above, and the reason the fix cannot simply be
    /// "always write the enum": proto3 JSON omits a field that carries its default,
    /// and the corpus's <c>GetInstance</c> request is the two bytes <c>{}</c>. A
    /// writer that emitted <c>"decision":"DECISION_KIND_UNSPECIFIED"</c> would break
    /// that fixture instead.
    /// </remarks>
    [Fact]
    public void csharp_json_omits_an_enum_left_at_its_zero_value()
    {
        var request = new Loams.Approvals.V1.DecideApprovalRequest { ApprovalId = "apr_01J9CREATED" };

        Assert.Equal("""{"approvalId":"apr_01J9CREATED"}""", CompactJson.Format(request));
    }

    /// <summary>
    /// An explicitly-set zero-valued enum with presence is written, not omitted.
    /// </summary>
    /// <remarks>
    /// Presence beats the default test, so this is the boundary between the two
    /// tests above. Without it, a caller who deliberately cleared an
    /// <c>optional</c> enum to <c>UNSPECIFIED</c> would have that choice dropped and
    /// the server would read the previous value.
    /// </remarks>
    [Fact]
    public void csharp_json_writes_an_optional_enum_set_to_zero()
    {
        // `loams.live.v1.QueryUpdate` has no optional enum, so the rule is pinned
        // through a message that does: `MutateRequest.idempotency_key` is an
        // `optional string`, and the same presence branch governs both.
        var request = new Loams.Live.V1.MutateRequest { IdempotencyKey = string.Empty };

        Assert.Equal("""{"idempotencyKey":""}""", CompactJson.Format(request));
    }

    /// <summary>
    /// A detail's base64 <c>value</c> is read whether or not it carries its
    /// <c>=</c> padding.
    /// </summary>
    /// <remarks>
    /// Every recorded <c>not_implemented</c> detail is
    /// <c>Cg9ub3RfaW1wbGVtZW50ZWQ</c> — 23 characters, one short of a multiple of
    /// four, and valid unpadded base64 for <c>0a 0f</c> + <c>not_implemented</c>.
    /// <c>Convert.FromBase64String</c> throws on it, and the throw was caught and
    /// turned into "no reason", so every structured-reason fixture in the corpus
    /// reported <c>reason: none</c> against a server that had said exactly which
    /// reason it meant. R8 is precisely about that reason reaching the caller.
    /// </remarks>
    [Fact]
    public void csharp_error_detail_reads_base64_without_padding()
    {
        const string padded = "Cg9ub3RfaW1wbGVtZW50ZWQ=";
        var body =
            $$"""{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.ErrorInfo","value":"{{padded[..^1]}}"}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));
        var info = WireReader.ErrorInfoFrom(parsed.Details);

        Assert.NotNull(info);
        Assert.Equal("not_implemented", info.Reason);
    }

    /// <summary>
    /// Padded base64 still decodes, so the fix is not "chop one character off".
    /// </summary>
    [Fact]
    public void csharp_error_detail_reads_padded_base64()
    {
        const string padded = "Cg9ub3RfaW1wbGVtZW50ZWQ=";
        var body =
            $$"""{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.ErrorInfo","value":"{{padded}}"}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));

        Assert.Equal("not_implemented", WireReader.ErrorInfoFrom(parsed.Details)?.Reason);
    }

    /// <summary>
    /// A <c>value</c> that is not base64 at all is still "no reason", and does not throw.
    /// </summary>
    /// <remarks>
    /// The padding fix must not turn a malformed detail into an exception: the
    /// documented answer for a detail this runtime cannot read is a failure with no
    /// <c>ErrorInfo</c>, which is what <see cref="WireReader.ErrorInfoFrom"/> has
    /// always promised.
    /// </remarks>
    [Fact]
    public void csharp_error_detail_rejects_base64_that_is_not_base64()
    {
        var body =
            """{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.ErrorInfo","value":"not base64 at all"}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));

        Assert.Null(WireReader.ErrorInfoFrom(parsed.Details));
    }

    /// <summary>A detail of another type is not read as an <c>ErrorInfo</c>, whatever it holds.</summary>
    [Fact]
    public void csharp_error_detail_ignores_a_detail_of_another_type()
    {
        var body =
            """{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.Other","value":"Cg9ub3RfaW1wbGVtZW50ZWQ="}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));

        Assert.Null(WireReader.ErrorInfoFrom(parsed.Details));
    }

    /// <summary>
    /// A <c>grpc-status-details-bin</c> that is not base64 costs the reason, never
    /// the refusal — and never an exception.
    /// </summary>
    /// <remarks>
    /// This one used to throw. The Connect detail path catches its
    /// <see cref="FormatException"/> and reports "no <c>ErrorInfo</c>", which is the
    /// documented answer; the gRPC-Web path did not catch anything, so a server that
    /// wrote that header as anything other than padded base64 would have thrown a
    /// <see cref="FormatException"/> out of the response reader — out of a refusal,
    /// which is the one thing a caller has to be able to catch.
    ///
    /// No recorded fixture covers it (every <c>grpc-status-details-bin</c> in
    /// <c>sdks/fixtures</c> happens to be padded), which is exactly why it needs a
    /// unit test rather than a fixture: a corpus is what the servers happened to
    /// send, and this is about what an SDK does with what they did not.
    /// </remarks>
    [Fact]
    public void csharp_error_trailer_survives_a_value_that_is_not_base64()
    {
        var trailers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["grpc-status"] = "12",
            ["grpc-message"] = "not implemented yet",
            ["grpc-status-details-bin"] = "not base64 at all",
        };

        var failure = WireReader.GrpcWebFailure(trailers, 200, "loams.instance.v1.InstanceService/WhoAmI");

        Assert.NotNull(failure);
        Assert.Equal(Code.Unimplemented, failure!.Code);
        Assert.Null(failure.Detail);
    }

    /// <summary>
    /// A padded <c>grpc-status-details-bin</c> still decodes, so the Connect and
    /// gRPC-Web paths agree on one base64 reader.
    /// </summary>
    [Fact]
    public void csharp_error_trailer_reads_padded_base64()
    {
        // A `google.rpc.Status` with code 12, no message, and one `Any` holding a
        // serialized `ErrorInfo` whose reason is `not_implemented`. Base64 of the
        // exact bytes, so this is the gRPC-Web spelling of the value the Connect
        // fixtures carry unpadded.
        const string statusBytes =
            "CAwaQgotdHlwZS5nb29nbGVhcGlzLmNvbS9sb2Ftcy5lcnJvcnMudjEuRXJyb3JJbmZvEhEKD25vdF9pbXBsZW1lbnRlZA==";
        var trailers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["grpc-status"] = "12",
            ["grpc-status-details-bin"] = statusBytes,
        };

        var failure = WireReader.GrpcWebFailure(trailers, 200, "loams.instance.v1.InstanceService/WhoAmI");

        Assert.NotNull(failure);
        Assert.Equal(Code.Unimplemented, failure!.Code);
        Assert.Equal("not_implemented", failure.Detail?.Reason);
    }

    /// <summary>
    /// A detail value that is base64 except for one leftover character is refused,
    /// because one sextet has no byte to stand for.
    /// </summary>
    /// <remarks>
    /// This is the boundary the padding fix must not slide past: "pad it and let
    /// <see cref="Convert"/> try" would decode this to a <i>shorter</i> but still
    /// plausible detail, which is how a reason goes missing without an error. The
    /// recorded corpus contains no such value, so the rule is pinned here.
    /// </remarks>
    [Fact]
    public void csharp_error_detail_rejects_base64_with_one_character_left_over()
    {
        const string padded = "Cg9ub3RfaW1wbGVtZW50ZWQ=";
        // One character short of the 20 the padding needs: a valid prefix, and not a
        // value any server writes.
        var body =
            $$"""{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.ErrorInfo","value":"{{padded[..1]}}"}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));

        Assert.Null(WireReader.ErrorInfoFrom(parsed.Details));
    }

    /// <summary>Base64 with whitespace inside it decodes, because a folded header carries it.</summary>
    [Fact]
    public void csharp_error_detail_reads_base64_with_whitespace()
    {
        var body =
            """{"code":"unimplemented","message":"m","details":[{"type":"loams.errors.v1.ErrorInfo","value":"Cg9u\nb3R faW1w\nbGVtZW50ZWQ"}]}""";

        Assert.True(WireReader.TryParseConnectErrorBody(body, out var parsed));

        Assert.Equal("not_implemented", WireReader.ErrorInfoFrom(parsed.Details)?.Reason);
    }
}
