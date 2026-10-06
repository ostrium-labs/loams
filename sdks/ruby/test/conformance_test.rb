# frozen_string_literal: true

require_relative "test_helper"

class ConformanceTest < Minitest::Test
  def test_ruby_conformance_all_required_fixtures
    corpus_dir = File.expand_path("../../fixtures", __dir__)
    driver = Loams::Test::CorpusDriver.new(corpus_dir)
    run_res = driver.run

    assert run_res.ok?, "Conformance run failed with problems:\n#{run_res.failures.join("\n")}"
    assert_equal driver.required_fixtures.size, run_res.ran.size, "all required fixtures must run"
  end

  def test_ruby_retry_reuses_idempotency_key
    decide = Loams::Descriptors.binding_for_rpc("loams.approvals.v1.ApprovalService/DecideApproval")
    refute_nil decide, "DecideApproval binding must exist"

    # Case 1: Mutation without key is not retried
    tp1 = Loams::Test::StubTransport.new
    tp1.answer_connect_error(503, "unavailable", "server down")
    tp1.answer_message(decide.response, decide.response.msgclass.new)

    req1 = decide.request.msgclass.new(approval_id: "apr_1")
    assert_raises(Loams::LoamsError) do
      tp1.client(max_retries: 3).invoker.unary(decide, req1, mint_idempotency_key: false)
    end
    assert_equal 1, tp1.requests.size, "mutation without key must not be retried"

    # Case 2: Keyed mutation is retried with the same key
    tp2 = Loams::Test::StubTransport.new
    tp2.answer_connect_error(503, "unavailable", "server down")
    tp2.answer_connect_error(503, "unavailable", "still down")
    tp2.answer_message(decide.response, decide.response.msgclass.new)

    req2 = decide.request.msgclass.new(approval_id: "apr_1")
    res2 = tp2.client(max_retries: 3).invoker.unary(decide, req2, mint_idempotency_key: true)
    refute_nil res2
    assert_equal 3, tp2.requests.size, "must make 3 attempts"

    keys = tp2.requests.map { |r| r.headers["idempotency-key"] || decide.request.msgclass.decode(r.body).idempotency_key }
    refute_empty keys.first, "first attempt must carry a key"
    assert_equal 1, keys.uniq.size, "all attempts must carry the EXACT SAME idempotency key"
  end

  def test_ruby_error_reason_mapping
    # Test unpadded base64 detail decoding
    not_impl_b64 = "Cg9ub3RfaW1wbGVtZW50ZWQ" # 23 chars, unpadded
    bytes = Loams::Base64Helper.decode(not_impl_b64)
    info = Loams::ErrorParser.decode_error_info_bytes(bytes)
    refute_nil info
    assert_equal "not_implemented", info.reason

    # Test Connect error mapping to typed class
    decide = Loams::Descriptors.binding_for_rpc("loams.approvals.v1.ApprovalService/DecideApproval")
    tp = Loams::Test::StubTransport.new
    tp.answer_connect_error(501, "unimplemented", "not implemented yet", not_impl_b64)

    err = assert_raises(Loams::LoamsError) do
      tp.client.invoker.unary(decide, decide.request.msgclass.new)
    end
    assert_equal :unimplemented, err.code
    assert_equal :not_implemented, err.reason
    assert_equal 501, err.http_status

    # Test unknown reason surfaced
    tp2 = Loams::Test::StubTransport.new
    io = StringIO.new
    Loams::Wire.write_bytes(io, 1, "custom_future_unknown_reason")
    custom_b64 = Loams::Base64Helper.encode(io.string)
    tp2.answer_connect_error(400, "invalid_argument", "unknown reason", custom_b64)

    err2 = assert_raises(Loams::LoamsError) do
      tp2.client.invoker.unary(decide, decide.request.msgclass.new)
    end
    assert_nil err2.reason
    assert_equal "custom_future_unknown_reason", err2.unknown_reason
  end

  def test_ruby_stream_resume_with_cursor
    watch = Loams::Descriptors.binding_for_rpc("loams.approvals.v1.ApprovalService/WatchApprovals")
    refute_nil watch

    # Build response frames
    # Frame 1: snapshot message with cursor "c1"
    resp_class = watch.response.msgclass
    m1 = resp_class.new(cursor: "c1", snapshot: resp_class.descriptor.lookup("snapshot").submsg_name ? Loams::Descriptors.message(resp_class.descriptor.lookup("snapshot").submsg_name)&.msgclass&.new : nil)
    # Frame 2: heartbeat message
    m2 = resp_class.new(heartbeat: resp_class.descriptor.lookup("heartbeat").submsg_name ? Loams::Descriptors.message(resp_class.descriptor.lookup("heartbeat").submsg_name)&.msgclass&.new : nil)

    tp = Loams::Test::StubTransport.new
    tp.answer_stream(
      Loams::MessageCodec.serialize(watch.response, m1, :proto),
      Loams::MessageCodec.serialize(watch.response, m2, :proto),
    )

    handle = tp.client.invoker.server_stream(watch, watch.request.msgclass.new)
    msgs = handle.messages.to_a

    assert_equal 1, msgs.size, "heartbeat must be filtered out of messages"
    assert_equal 1, handle.heartbeats, "heartbeat must be counted"
    assert_equal 2, handle.frame_kinds.size, "both frame kinds recorded"
    assert_equal %w[snapshot heartbeat], handle.frame_kinds
  end

  def test_ruby_token_source_refresh
    decide = Loams::Descriptors.binding_for_rpc("loams.approvals.v1.ApprovalService/DecideApproval")
    
    tokens = %w[initial-expired refreshed-valid]
    source = Loams::TokenSource.new { tokens.shift }

    tp = Loams::Test::StubTransport.new
    tp.answer_connect_error(401, "unauthenticated", "token expired")
    tp.answer_message(decide.response, decide.response.msgclass.new)

    client = tp.client(token_source: source)
    res = client.invoker.unary(decide, decide.request.msgclass.new)
    refute_nil res

    assert_equal 2, tp.requests.size, "must retry after token refresh"
    assert_equal "Bearer initial-expired", tp.requests[0].headers["authorization"]
    assert_equal "Bearer refreshed-valid", tp.requests[1].headers["authorization"]
  end

  def test_ruby_pagination_iterator
    list = Loams::Descriptors.binding_for_rpc("loams.approvals.v1.ApprovalService/ListApprovals")
    refute_nil list

    resp_class = list.response.msgclass
    approval_class = Loams::Descriptors.message("loams.approvals.v1.Approval")&.msgclass

    # Mock 3 pages
    p1 = resp_class.new(approvals: [approval_class.new(id: "apr_1"), approval_class.new(id: "apr_2")], next_page_token: "page2")
    p2 = resp_class.new(approvals: [approval_class.new(id: "apr_3")], next_page_token: "page3")
    p3 = resp_class.new(approvals: [approval_class.new(id: "apr_4")], next_page_token: "")

    tp = Loams::Test::StubTransport.new
    tp.answer_message(list.response, p1)
    tp.answer_message(list.response, p2)
    tp.answer_message(list.response, p3)

    iterator = Loams::PageIterator.new(
      fetch: proc { |token| tp.client.invoker.unary(list, list.request.msgclass.new(page_token: token)) },
      items_field: list.response.lookup("approvals"),
      next_page_token_field: list.response.lookup("next_page_token"),
    )

    items = iterator.to_a
    assert_equal 4, items.size
    assert_equal %w[apr_1 apr_2 apr_3 apr_4], items.map(&:id)
    assert_equal 3, tp.requests.size
  end
end
