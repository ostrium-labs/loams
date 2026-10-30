# frozen_string_literal: true

require "json"
require "loams/wire"

module Loams
  REASONS = [
    :approval_expired,
    :approval_already_decided,
    :approval_stale_revision,
    :requester_cannot_approve,
    :decision_proof_invalid,
    :step_up_required,
    :reason_required,
    :invalid_decision,
    :pairing_expired,
    :pairing_used,
    :device_revoked,
    :push_target_unknown,
    :not_implemented,
    :feature_not_in_variant,
    :house_not_configured,
    :house_syntax_error,
    :house_unknown_table,
    :house_unknown_database,
    :house_access_denied,
    :house_read_only,
    :house_timeout,
    :house_memory_limit,
    :house_too_many_queries,
    :house_quota_exceeded,
    :house_disabled_function,
    :house_cancelled,
    :house_query_not_found,
    :house_query_failed,
    :project_not_found,
    :branch_has_children,
    :branch_protected,
    :lsn_out_of_retention,
    :endpoint_exists_for_branch,
    :compute_start_failed,
    :unsupported_pg_version,
    :beta_major_requires_opt_in,
    :compute_major_mismatch_refused,
    :import_checksums_enabled,
    :quota_exceeded,
    :storage_unavailable,
    :secret_already_issued,
    :repo_not_found,
    :repo_already_exists,
    :repo_deleting,
    :ref_stale,
    :ref_protected,
    :out_of_scope,
    :object_format_unsupported,
    :fork_too_deep,
    :sequencer_unavailable,
    :idempotency_key_reused,
    :workspace_not_found,
    :workspace_fenced,
    :batch_partially_applied,
    :workspace_read_only,
    :workspace_name_taken,
    :object_missing,
    :secret_detected,
    :env_unsealed,
    :env_access_denied,
    :env_foreign_reference,
    :memory_store_not_found,
    :memory_space_not_found,
    :memory_space_limit,
    :memory_managed_object,
    :memory_block_version_mismatch,
    :memory_block_too_large,
    :memory_core_budget_exceeded,
    :memory_content_too_large,
    :memory_secret_detected,
    :memory_pii_refused,
    :memory_model_not_configured,
    :memory_model_not_local,
    :memory_token_budget_exhausted,
    :memory_extraction_failed,
    :memory_too_many_spaces,
    :memory_cross_space_write,
    :memory_not_yet_available,
    :code_repo_not_indexed,
    :code_index_building,
    :code_change_too_large,
    :code_language_unsupported,
    :code_scip_invalid,
    :live_bad_cursor,
    :live_worker_crashed,
    :live_deploy_conflict,
    :live_schema_violation,
    :filter_conflict,
    :search_after_needs_tiebreak,
    :invalid_consistency_token,
    :namespace_required,
    :sql_param_count,
    :live_index_not_ready,
    :live_app_disabled,
    :live_app_exists,
    :live_function_error,
    :live_function_timeout,
    :client_deadline_exceeded,
    :live_function_out_of_memory,
    :live_limit_exceeded,
    :result_too_large,
    :live_app_mismatch,
    :live_origin_not_allowed,
    :session_rotate,
    :session_gone,
    :node_draining,
    :role_required,
    :setup_required,
    :token_missing,
    :token_malformed,
    :token_revoked,
    :token_not_yet_valid,
    :token_bad_signature,
    :token_wrong_audience,
    :token_wrong_issuer,
    :token_unknown_key,
    :token_algorithm_not_allowed,
    :api_key_invalid,
    :credential_not_accepted,
    :credential_in_query_string,
    :csrf_token_invalid,
    :principal_not_allowed,
    :cors_origin_not_allowed,
    :auth_unavailable,
    :join_token_unknown,
    :join_token_spent,
    :join_token_expired,
    :identity_not_granted,
    :identity_revoked,
    :ca_not_initialized,
    :invalid_argument,
    :not_found,
    :already_exists,
    :permission_denied,
    :token_expired,
    :unauthenticated,
    :failed_precondition,
    :resource_exhausted,
    :unavailable,
    :deadline_exceeded,
    :aborted,
    :internal,
  ].freeze

  REASON_CODES = {
    approval_expired: :failed_precondition,
    approval_already_decided: :failed_precondition,
    approval_stale_revision: :failed_precondition,
    requester_cannot_approve: :permission_denied,
    decision_proof_invalid: :permission_denied,
    step_up_required: :unauthenticated,
    reason_required: :invalid_argument,
    invalid_decision: :invalid_argument,
    pairing_expired: :failed_precondition,
    pairing_used: :failed_precondition,
    device_revoked: :unauthenticated,
    push_target_unknown: :not_found,
    not_implemented: :unimplemented,
    feature_not_in_variant: :unimplemented,
    house_not_configured: :unimplemented,
    house_syntax_error: :invalid_argument,
    house_unknown_table: :not_found,
    house_unknown_database: :not_found,
    house_access_denied: :permission_denied,
    house_read_only: :failed_precondition,
    house_timeout: :deadline_exceeded,
    house_memory_limit: :resource_exhausted,
    house_too_many_queries: :resource_exhausted,
    house_quota_exceeded: :resource_exhausted,
    house_disabled_function: :permission_denied,
    house_cancelled: :canceled,
    house_query_not_found: :not_found,
    house_query_failed: :unknown,
    project_not_found: :not_found,
    branch_has_children: :failed_precondition,
    branch_protected: :failed_precondition,
    beta_major_requires_opt_in: :failed_precondition,
    compute_major_mismatch_refused: :failed_precondition,
    import_checksums_enabled: :failed_precondition,
    lsn_out_of_retention: :failed_precondition,
    endpoint_exists_for_branch: :already_exists,
    compute_start_failed: :unavailable,
    quota_exceeded: :resource_exhausted,
    storage_unavailable: :unavailable,
    secret_already_issued: :failed_precondition,
    repo_not_found: :not_found,
    repo_already_exists: :already_exists,
    repo_deleting: :failed_precondition,
    ref_stale: :aborted,
    ref_protected: :failed_precondition,
    out_of_scope: :permission_denied,
    object_format_unsupported: :unimplemented,
    fork_too_deep: :failed_precondition,
    sequencer_unavailable: :unavailable,
    idempotency_key_reused: :failed_precondition,
    workspace_not_found: :not_found,
    workspace_fenced: :aborted,
    batch_partially_applied: :aborted,
    workspace_read_only: :failed_precondition,
    workspace_name_taken: :already_exists,
    object_missing: :failed_precondition,
    secret_detected: :failed_precondition,
    env_unsealed: :invalid_argument,
    env_access_denied: :permission_denied,
    env_foreign_reference: :permission_denied,
    memory_store_not_found: :not_found,
    memory_space_not_found: :not_found,
    memory_space_limit: :resource_exhausted,
    memory_managed_object: :failed_precondition,
    memory_block_version_mismatch: :aborted,
    memory_block_too_large: :invalid_argument,
    memory_core_budget_exceeded: :resource_exhausted,
    memory_content_too_large: :invalid_argument,
    memory_secret_detected: :failed_precondition,
    memory_pii_refused: :failed_precondition,
    memory_model_not_configured: :failed_precondition,
    memory_model_not_local: :failed_precondition,
    memory_token_budget_exhausted: :resource_exhausted,
    memory_extraction_failed: :failed_precondition,
    memory_too_many_spaces: :invalid_argument,
    memory_cross_space_write: :invalid_argument,
    memory_not_yet_available: :unimplemented,
    code_repo_not_indexed: :failed_precondition,
    code_index_building: :unavailable,
    code_change_too_large: :invalid_argument,
    code_language_unsupported: :failed_precondition,
    code_scip_invalid: :invalid_argument,
    live_bad_cursor: :invalid_argument,
    live_worker_crashed: :internal,
    live_deploy_conflict: :failed_precondition,
    live_schema_violation: :invalid_argument,
    filter_conflict: :invalid_argument,
    search_after_needs_tiebreak: :invalid_argument,
    invalid_consistency_token: :invalid_argument,
    namespace_required: :invalid_argument,
    sql_param_count: :invalid_argument,
    unsupported_pg_version: :invalid_argument,
    live_index_not_ready: :failed_precondition,
    live_app_disabled: :failed_precondition,
    live_app_exists: :failed_precondition,
    live_function_error: :unknown,
    live_function_timeout: :deadline_exceeded,
    client_deadline_exceeded: :deadline_exceeded,
    live_function_out_of_memory: :resource_exhausted,
    live_limit_exceeded: :resource_exhausted,
    result_too_large: :resource_exhausted,
    live_app_mismatch: :permission_denied,
    live_origin_not_allowed: :permission_denied,
    session_rotate: :unavailable,
    session_gone: :not_found,
    node_draining: :unavailable,
    role_required: :permission_denied,
    setup_required: :failed_precondition,
    token_missing: :unauthenticated,
    token_malformed: :unauthenticated,
    token_revoked: :unauthenticated,
    token_not_yet_valid: :unauthenticated,
    token_bad_signature: :unauthenticated,
    token_wrong_audience: :unauthenticated,
    token_wrong_issuer: :unauthenticated,
    token_unknown_key: :unauthenticated,
    token_algorithm_not_allowed: :unauthenticated,
    api_key_invalid: :unauthenticated,
    credential_not_accepted: :unauthenticated,
    credential_in_query_string: :unauthenticated,
    csrf_token_invalid: :permission_denied,
    principal_not_allowed: :permission_denied,
    cors_origin_not_allowed: :permission_denied,
    auth_unavailable: :unavailable,
    join_token_unknown: :unauthenticated,
    join_token_spent: :unauthenticated,
    join_token_expired: :unauthenticated,
    identity_not_granted: :permission_denied,
    identity_revoked: :permission_denied,
    ca_not_initialized: :failed_precondition,
    invalid_argument: :invalid_argument,
    not_found: :not_found,
    already_exists: :already_exists,
    permission_denied: :permission_denied,
    token_expired: :unauthenticated,
    unauthenticated: :unauthenticated,
    failed_precondition: :failed_precondition,
    resource_exhausted: :resource_exhausted,
    unavailable: :unavailable,
    deadline_exceeded: :deadline_exceeded,
    aborted: :aborted,
    internal: :internal,
  }.freeze

  HTTP_TO_CODE = {
    400 => :invalid_argument,
    401 => :unauthenticated,
    403 => :permission_denied,
    404 => :not_found,
    409 => :already_exists,
    429 => :resource_exhausted,
    499 => :canceled,
    500 => :internal,
    501 => :unimplemented,
    502 => :unavailable,
    503 => :unavailable,
    504 => :deadline_exceeded,
  }.freeze

  GRPC_TO_CODE = {
    1  => :canceled,
    2  => :unknown,
    3  => :invalid_argument,
    4  => :deadline_exceeded,
    5  => :not_found,
    6  => :already_exists,
    7  => :permission_denied,
    8  => :resource_exhausted,
    9  => :failed_precondition,
    10 => :aborted,
    11 => :out_of_range,
    12 => :unimplemented,
    13 => :internal,
    14 => :unavailable,
    15 => :data_loss,
    16 => :unauthenticated,
  }.freeze

  CODE_TO_HTTP = {
    canceled: 499,
    unknown: 500,
    invalid_argument: 400,
    deadline_exceeded: 504,
    not_found: 404,
    already_exists: 409,
    permission_denied: 403,
    resource_exhausted: 429,
    failed_precondition: 400,
    aborted: 409,
    out_of_range: 400,
    unimplemented: 501,
    internal: 500,
    unavailable: 503,
    data_loss: 500,
    unauthenticated: 401,
  }.freeze

  # ErrorInfo struct representing decoded detail
  ErrorInfo = Struct.new(:reason, :domain, :metadata, :hint, keyword_init: true)

  class LoamsError < StandardError
    attr_reader :code, :reason, :unknown_reason, :http_status, :metadata, :hint, :details, :rpc

    def initialize(
      message = nil,
      code: :unknown,
      reason: nil,
      unknown_reason: nil,
      http_status: nil,
      metadata: {},
      hint: "",
      details: nil,
      rpc: nil
    )
      @code = code.to_sym
      @reason = reason ? reason.to_sym : nil
      @unknown_reason = unknown_reason
      @http_status = http_status || Loams::CODE_TO_HTTP[@code]
      @metadata = metadata || {}
      @hint = hint || ""
      @details = details
      @rpc = rpc

      msg = message || default_message
      super(msg)
    end

    def default_message
      parts = ["[#{@code}]"]
      parts << "reason=#{@reason || @unknown_reason}" if @reason || @unknown_reason
      parts << "rpc=#{@rpc}" if @rpc
      parts.join(" ")
    end

    def retryable?
      [429, 502, 503, 504].include?(@http_status) || [:unavailable, :resource_exhausted].include?(@code)
    end
  end

  class CanceledError < LoamsError; end
  class UnknownError < LoamsError; end
  class InvalidArgumentError < LoamsError; end
  class DeadlineExceededError < LoamsError; end
  class NotFoundError < LoamsError; end
  class AlreadyExistsError < LoamsError; end
  class PermissionDeniedError < LoamsError; end
  class ResourceExhaustedError < LoamsError; end
  class FailedPreconditionError < LoamsError; end
  class AbortedError < LoamsError; end
  class OutOfRangeError < LoamsError; end
  class UnimplementedError < LoamsError; end
  class InternalError < LoamsError; end
  class UnavailableError < LoamsError; end
  class DataLossError < LoamsError; end
  class UnauthenticatedError < LoamsError; end

  # Specialised subclasses for distinct reason handling
  class TokenExpiredError < UnauthenticatedError; end
  class FeatureNotInVariantError < UnimplementedError; end

  CODE_TO_CLASS = {
    canceled: CanceledError,
    unknown: UnknownError,
    invalid_argument: InvalidArgumentError,
    deadline_exceeded: DeadlineExceededError,
    not_found: NotFoundError,
    already_exists: AlreadyExistsError,
    permission_denied: PermissionDeniedError,
    resource_exhausted: ResourceExhaustedError,
    failed_precondition: FailedPreconditionError,
    aborted: AbortedError,
    out_of_range: OutOfRangeError,
    unimplemented: UnimplementedError,
    internal: InternalError,
    unavailable: UnavailableError,
    data_loss: DataLossError,
    unauthenticated: UnauthenticatedError,
  }.freeze

  module Base64Helper
    def self.decode(str)
      return "" if str.nil? || str.empty?
      clean = str.tr("-_", "+/").gsub(/\s+/, "")
      padded = clean + ("=" * ((4 - (clean.length % 4)) % 4))
      padded.unpack1("m0") rescue nil
    end

    def self.encode(bytes)
      return "" if bytes.nil? || bytes.empty?
      [bytes].pack("m0").strip
    end
  end

  module ErrorParser
    # Decodes an ErrorInfo protobuf from raw bytes (either raw or wrapped in Status)
    def self.decode_error_info_bytes(bytes)
      return nil if bytes.nil? || bytes.empty?

      begin
        # Check if wrapped in google.rpc.Status (field 3 repeated Any)
        Loams::Wire.each_field(bytes) do |field, wt, val|
          if field == 3 && wt == 2
            type_url = Loams::Wire.find_bytes(val, 1)
            payload = Loams::Wire.find_bytes(val, 2)
            if payload && (type_url.nil? || type_url.include?("ErrorInfo"))
              return parse_raw_error_info(payload)
            end
          end
        end

        # Fallback: raw ErrorInfo
        parse_raw_error_info(bytes)
      rescue StandardError
        nil
      end
    end

    def self.parse_raw_error_info(payload)
      reason = Loams::Wire.find_bytes(payload, 1)
      domain = Loams::Wire.find_bytes(payload, 2)
      metadata = {}
      # field 3: map<string, string> -> message with field 1 key, field 2 value
      Loams::Wire.each_field(payload) do |f, wt, val|
        if f == 3 && wt == 2
          k = Loams::Wire.find_bytes(val, 1)
          v = Loams::Wire.find_bytes(val, 2)
          metadata[k] = v if k
        end
      end
      # hint might be field 4 or similar if defined
      hint = Loams::Wire.find_bytes(payload, 4) || ""

      return nil if reason.nil? && domain.nil?

      ErrorInfo.new(reason: reason, domain: domain, metadata: metadata, hint: hint)
    end

    # Parses a Connect JSON error body or trailers
    def self.parse_connect_error(body, status = nil, rpc = nil)
      parsed = JSON.parse(body) rescue {}
      parsed = parsed["error"] if parsed["error"].is_a?(Hash)

      code_str = parsed["code"]
      message = parsed["message"] || "Connect error"

      code = code_str ? code_str.to_sym : (status ? HTTP_TO_CODE[status] || :unknown : :unknown)

      error_info = nil
      if parsed["details"].is_a?(Array)
        parsed["details"].each do |d|
          next unless d.is_a?(Hash)

          if d["debug"].is_a?(Hash) && d["debug"]["reason"]
            error_info = ErrorInfo.new(
              reason: d["debug"]["reason"],
              domain: d["debug"]["domain"],
              metadata: d["debug"]["metadata"] || {},
              hint: d["debug"]["hint"] || "",
            )
            break
          elsif d["value"].is_a?(String)
            bytes = Base64Helper.decode(d["value"])
            if bytes
              info = decode_error_info_bytes(bytes)
              if info
                error_info = info
                break
              end
            end
          elsif d["reason"].is_a?(String)
            error_info = ErrorInfo.new(
              reason: d["reason"],
              domain: d["domain"],
              metadata: d["metadata"] || {},
              hint: d["hint"] || "",
            )
            break
          end
        end
      end

      build_error(code, message, error_info, status, rpc, parsed["details"])
    end

    # Parses gRPC-Web trailers
    def self.parse_grpc_trailers(headers, status = nil, rpc = nil)
      grpc_status = headers["grpc-status"] || headers["Grpc-Status"]
      grpc_message = headers["grpc-message"] || headers["Grpc-Message"]
      status_bin = headers["grpc-status-details-bin"] || headers["Grpc-Status-Details-Bin"]

      code_num = grpc_status ? grpc_status.to_i : 2
      code = GRPC_TO_CODE[code_num] || :unknown
      message = grpc_message ? URI.decode_www_form_component(grpc_message) : "gRPC error #{code_num}"

      error_info = nil
      if status_bin
        bytes = Base64Helper.decode(status_bin)
        error_info = decode_error_info_bytes(bytes) if bytes
      end

      build_error(code, message, error_info, status, rpc)
    end

    def self.build_error(code, message, error_info, status, rpc, details = nil)
      raw_reason = error_info&.reason
      reason_sym = nil
      unknown_reason = nil

      if raw_reason
        sym = raw_reason.to_sym
        if REASONS.include?(sym)
          reason_sym = sym
        else
          unknown_reason = raw_reason
        end
      end

      # Pick specific subclass if applicable
      klass = if reason_sym == :token_expired
        TokenExpiredError
      elsif reason_sym == :feature_not_in_variant
        FeatureNotInVariantError
      else
        CODE_TO_CLASS[code] || LoamsError
      end

      klass.new(
        message,
        code: code,
        reason: reason_sym,
        unknown_reason: unknown_reason,
        http_status: status,
        metadata: error_info&.metadata || {},
        hint: error_info&.hint || "",
        details: details,
        rpc: rpc,
      )
    end
  end
end
