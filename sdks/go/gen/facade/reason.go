package facade

import "sort"

// Reason is a stable, machine-readable cause of a failed RPC
// (design §44 §7.4, D611). It is generated from `docs/api/reasons.md`, so a
// caller switching on it is exhaustive over the registry this SDK was built
// against, and a reason the registry has lost stops compiling.
//
// It is a distinct string type rather than `string` on purpose: that is what
// makes `errors.As`-free exhaustiveness possible, and it is why the wire value
// has to go through `KnownReason` before it becomes a `Reason`.
type Reason string

// Every reason in the registry. Transcribed from `docs/api/reasons.md`; the test
// `TestReasonRegistryMatchesDocs` reads that page and fails on a drift, so this
// list cannot silently fall behind.
const (
	ReasonApprovalExpired         Reason = "approval_expired"
	ReasonApprovalAlreadyDecided  Reason = "approval_already_decided"
	ReasonApprovalStaleRevision   Reason = "approval_stale_revision"
	ReasonRequesterCannotApprove  Reason = "requester_cannot_approve"
	ReasonDecisionProofInvalid    Reason = "decision_proof_invalid"
	ReasonStepUpRequired          Reason = "step_up_required"
	ReasonReasonRequired          Reason = "reason_required"
	ReasonInvalidDecision         Reason = "invalid_decision"
	ReasonPairingExpired          Reason = "pairing_expired"
	ReasonPairingUsed             Reason = "pairing_used"
	ReasonDeviceRevoked           Reason = "device_revoked"
	ReasonPushTargetUnknown       Reason = "push_target_unknown"
	ReasonNotImplemented          Reason = "not_implemented"
	ReasonFeatureNotInVariant     Reason = "feature_not_in_variant"
	ReasonProjectNotFound         Reason = "project_not_found"
	ReasonBranchHasChildren       Reason = "branch_has_children"
	ReasonBranchProtected         Reason = "branch_protected"
	ReasonLsnOutOfRetention       Reason = "lsn_out_of_retention"
	ReasonEndpointExistsForBranch Reason = "endpoint_exists_for_branch"
	ReasonComputeStartFailed      Reason = "compute_start_failed"
	ReasonQuotaExceeded           Reason = "quota_exceeded"
	ReasonStorageUnavailable      Reason = "storage_unavailable"
	ReasonSecretAlreadyIssued     Reason = "secret_already_issued"
	ReasonInvalidArgument         Reason = "invalid_argument"
	ReasonNotFound                Reason = "not_found"
	ReasonAlreadyExists           Reason = "already_exists"
	ReasonPermissionDenied        Reason = "permission_denied"
	ReasonTokenExpired            Reason = "token_expired"
	ReasonUnauthenticated         Reason = "unauthenticated"
	ReasonFailedPrecondition      Reason = "failed_precondition"
	ReasonResourceExhausted       Reason = "resource_exhausted"
	ReasonUnavailable             Reason = "unavailable"
	ReasonDeadlineExceeded        Reason = "deadline_exceeded"
	ReasonAborted                 Reason = "aborted"
	ReasonInternal                Reason = "internal"
)

// FeatureNotInVariant is the reason a package the build variant does not carry
// answers with (design §44 §4, D600). Every RPC of such a package refuses with
// it and names the variant in `metadata.variant`; the runtime turns that refusal
// into `loams.FeatureNotInVariantError`, so a caller can branch on the type —
// or on this reason — without having to call and read the message.
const FeatureNotInVariant = ReasonFeatureNotInVariant

// Reasons is every reason in the registry, in the registry's order.
var Reasons = []Reason{
	ReasonApprovalExpired,
	ReasonApprovalAlreadyDecided,
	ReasonApprovalStaleRevision,
	ReasonRequesterCannotApprove,
	ReasonDecisionProofInvalid,
	ReasonStepUpRequired,
	ReasonReasonRequired,
	ReasonInvalidDecision,
	ReasonPairingExpired,
	ReasonPairingUsed,
	ReasonDeviceRevoked,
	ReasonPushTargetUnknown,
	ReasonNotImplemented,
	ReasonFeatureNotInVariant,
	ReasonProjectNotFound,
	ReasonBranchHasChildren,
	ReasonBranchProtected,
	ReasonLsnOutOfRetention,
	ReasonEndpointExistsForBranch,
	ReasonComputeStartFailed,
	ReasonQuotaExceeded,
	ReasonStorageUnavailable,
	ReasonSecretAlreadyIssued,
	ReasonInvalidArgument,
	ReasonNotFound,
	ReasonAlreadyExists,
	ReasonPermissionDenied,
	ReasonTokenExpired,
	ReasonUnauthenticated,
	ReasonFailedPrecondition,
	ReasonResourceExhausted,
	ReasonUnavailable,
	ReasonDeadlineExceeded,
	ReasonAborted,
	ReasonInternal,
}

// ReasonCodes is the Connect code each reason is raised under, from the same
// registry. It is a `connect.Code`, which is the SDK's `loams.Code`, so a
// caller that wants the coarse class has it without string parsing.
var ReasonCodes = map[Reason]string{
	ReasonApprovalExpired:         "failed_precondition",
	ReasonApprovalAlreadyDecided:  "failed_precondition",
	ReasonApprovalStaleRevision:   "failed_precondition",
	ReasonRequesterCannotApprove:  "permission_denied",
	ReasonDecisionProofInvalid:    "permission_denied",
	ReasonStepUpRequired:          "unauthenticated",
	ReasonReasonRequired:          "invalid_argument",
	ReasonInvalidDecision:         "invalid_argument",
	ReasonPairingExpired:          "failed_precondition",
	ReasonPairingUsed:             "failed_precondition",
	ReasonDeviceRevoked:           "unauthenticated",
	ReasonPushTargetUnknown:       "not_found",
	ReasonNotImplemented:          "unimplemented",
	ReasonFeatureNotInVariant:     "unimplemented",
	ReasonProjectNotFound:         "not_found",
	ReasonBranchHasChildren:       "failed_precondition",
	ReasonBranchProtected:         "failed_precondition",
	ReasonLsnOutOfRetention:       "failed_precondition",
	ReasonEndpointExistsForBranch: "already_exists",
	ReasonComputeStartFailed:      "unavailable",
	ReasonQuotaExceeded:           "resource_exhausted",
	ReasonStorageUnavailable:      "unavailable",
	ReasonSecretAlreadyIssued:     "failed_precondition",
	ReasonInvalidArgument:         "invalid_argument",
	ReasonNotFound:                "not_found",
	ReasonAlreadyExists:           "already_exists",
	ReasonPermissionDenied:        "permission_denied",
	ReasonTokenExpired:            "unauthenticated",
	ReasonUnauthenticated:         "unauthenticated",
	ReasonFailedPrecondition:      "failed_precondition",
	ReasonResourceExhausted:       "resource_exhausted",
	ReasonUnavailable:             "unavailable",
	ReasonDeadlineExceeded:        "deadline_exceeded",
	ReasonAborted:                 "aborted",
	ReasonInternal:                "internal",
}

var reasonSet = func() map[Reason]struct{} {
	set := make(map[Reason]struct{}, len(Reasons))
	for _, reason := range Reasons {
		set[reason] = struct{}{}
	}
	return set
}()

// KnownReason turns a wire string into a Reason. The second result is false for
// a reason this SDK's registry does not have — a server newer than this SDK —
// which the runtime surfaces rather than drops (design §44 §7.4, R8: losing it
// would leave a caller unable to tell "not supported here" from "not supported
// at all").
func KnownReason(value string) (Reason, bool) {
	reason := Reason(value)
	_, known := reasonSet[reason]
	return reason, known
}

// IsKnownReason whether a reason is in this SDK's registry.
func IsKnownReason(reason Reason) bool {
	_, known := reasonSet[reason]
	return known
}

// Binding finds the binding a module and call name identify. `call` is the Go
// method name (`GetInstance`), which is what the module methods are named after.
func Binding(module, call string) (CallBinding, bool) {
	for _, entry := range Modules {
		if entry.Name != module {
			continue
		}
		for _, candidate := range entry.Calls {
			if candidate.Name == call || candidate.ProtoName == call {
				return candidate, true
			}
		}
	}
	return CallBinding{}, false
}

// Module finds a module binding by name.
func Module(name string) (ModuleBinding, bool) {
	for _, entry := range Modules {
		if entry.Name == name {
			return entry, true
		}
	}
	return ModuleBinding{}, false
}

// PackageOf answers which module owns a proto package. `loams.live` and
// `loams.tables` are two names for one package, so the first module in
// registration order that claims it wins; use `Modules` when the whole list of
// facade names matters.
func PackageOf(pkg string) (string, bool) {
	for _, entry := range Modules {
		if entry.Package == pkg {
			return entry.Name, true
		}
	}
	return "", false
}

// ModulesForPackage is every module name that is a facade for one proto
// package. It is what `loams.System().Guard` consults, because a guard on
// "live" must also cover "tables" and a refusal on either means the engine is
// absent.
func ModulesForPackage(pkg string) []string {
	var names []string
	for _, entry := range Modules {
		if entry.Package == pkg {
			names = append(names, entry.Name)
		}
	}
	sort.Strings(names)
	return names
}
