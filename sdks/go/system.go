// Feature detection and version reporting (design §44 §4, D600; runtime
// contract R5 and R9).
//
// Two halves of R5, and an SDK needs both.
//
//  1. **Without calling.** `GetInstance.Services[]` says which packages this
//     binary carries. One call, no auth, cheap. `Available`, `Served`,
//     `Unavailable` and `Guard` wrap it; the catalogue is cached for the life of
//     the process and concurrent readers share one in-flight fetch, so a
//     hundred goroutines asking at once cost one call.
//  2. **When the caller calls anyway.** Every RPC of an absent package answers
//     `unimplemented` with `reason = feature_not_in_variant` and the variant in
//     `metadata.variant`. The runtime turns that into `FeatureNotInVariantError`
//     (see `errors.go`), so the branch is a type check and never the package
//     name, which is a proto detail, and never the message.
//
// `Guard` raises the *same* error type, so one `errors.As` covers "the guard
// said no" and "the server refused", and the guard costs no RPC once the
// catalogue is cached.

package loams

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"loams.dev/go/gen/facade"
)

// ServiceStatus is one package of the API catalogue and what this binary does
// with it, as `GetInstance.Services[]` reports it.
type ServiceStatus struct {
	// Package is the proto package, for example "loams.live.v1".
	Package string
	// Version is the package's API version, for example "v1".
	Version string
	// Available is whether this binary serves the package. False means every
	// one of its RPCs answers `unimplemented` with reason
	// `feature_not_in_variant`.
	Available bool
	// Services are the fully qualified service names in the package.
	Services []string
	// Unstable is true for a package whose wire contract may still change.
	Unstable bool
}

// Catalogue is the module catalogue, as one call reports it.
type Catalogue struct {
	// Served are the packages this binary serves.
	Served []string
	// Unavailable are the packages it knows about and does not serve.
	Unavailable []string
	// Missing are the packages this SDK speaks that the instance does not list
	// at all — a package whose services have not been defined yet, which is
	// different from one that exists and is switched off.
	Missing []string
	// Services is every entry, in the server's order.
	Services []ServiceStatus
}

// VersionReport is what the SDK speaks beside what the server serves (R9).
type VersionReport struct {
	// ProtoRev is the proto revision this SDK was generated from.
	ProtoRev string
	// ServerVersion is the server's own semver, as `GetInstance` reports it.
	ServerVersion string
	// APIVersions is the server's `GetInstance.ApiVersions`: only what it
	// serves, so a package this SDK speaks and the server does not is missing
	// rather than reported as available.
	APIVersions []string
	// Compatible is true when every package the SDK speaks is served.
	Compatible bool
	// Missing are the SDK's packages the server does not serve.
	Missing []string
}

// System is the module catalogue, feature detection and the version check.
type System struct {
	client   *Client
	endpoint string
	// speaks is every proto package this SDK speaks, which is what R9's report
	// compares the server's `api_versions` against. It is computed once at
	// construction: it is a property of the SDK, not of the server.
	speaks []string

	// mu guards the cached catalogue and the in-flight fetch. A single mutex
	// with a condition-free hand-off: the fetch is started under the lock and
	// the result is stored under it, so concurrent readers block on the mutex
	// for the duration of a network call. That is the wrong shape for a slow
	// call, so `inFlight` carries the completion and readers wait on it.
	mu        sync.Mutex
	catalogue *Catalogue
	inFlight  chan struct{}
}

// Endpoint is the instance this client talks to.
func (s *System) Endpoint() string { return s.endpoint }

// Invalidate forgets the cached catalogue, so the next check calls again.
func (s *System) Invalidate() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.catalogue = nil
}

// Catalogue returns the module catalogue, fetching it once and sharing the
// result with every concurrent reader.
//
// `LOAMS_TEST_ENDPOINT` and a `loams dev` are the same shape here: one
// `GetInstance` call, no auth.
func (s *System) Catalogue(ctx context.Context) (*Catalogue, error) {
	s.mu.Lock()
	if s.catalogue != nil {
		catalogue := s.catalogue
		s.mu.Unlock()
		return catalogue, nil
	}
	if inFlight := s.inFlight; inFlight != nil {
		s.mu.Unlock()
		select {
		case <-inFlight:
		case <-ctx.Done():
			return nil, ToLoamsError(ctx.Err(), instanceGetInstanceRPC)
		}
		s.mu.Lock()
		catalogue := s.catalogue
		s.mu.Unlock()
		if catalogue == nil {
			return nil, ToLoamsError(context.Canceled, instanceGetInstanceRPC)
		}
		return catalogue, nil
	}
	inFlight := make(chan struct{})
	s.inFlight = inFlight
	s.mu.Unlock()

	info, err := s.client.Instance().GetInstance(ctx, &GetInstanceRequest{})
	s.mu.Lock()
	defer s.mu.Unlock()
	close(inFlight)
	s.inFlight = nil
	if err != nil {
		return nil, err
	}
	catalogue := s.catalogueFrom(info)
	s.catalogue = catalogue
	return catalogue, nil
}

const instanceGetInstanceRPC = "loams.instance.v1.InstanceService/GetInstance"

// catalogueFrom turns a GetInstance response into the catalogue.
func (s *System) catalogueFrom(info *GetInstanceResponse) *Catalogue {
	catalogue := &Catalogue{}
	for _, service := range info.GetServices() {
		status := ServiceStatus{
			Package:   service.GetPackage(),
			Version:   service.GetVersion(),
			Available: service.GetAvailable(),
			Services:  append([]string(nil), service.GetServices()...),
			Unstable:  service.GetUnstable(),
		}
		catalogue.Services = append(catalogue.Services, status)
		if status.Available {
			catalogue.Served = append(catalogue.Served, status.Package)
		} else {
			catalogue.Unavailable = append(catalogue.Unavailable, status.Package)
		}
	}
	// The SDK's own packages that the server does not list at all. A package
	// whose services have not been defined yet has no entry, so this is
	// different from one that exists and is switched off, and conflating the two
	// would report a server bug as a missing feature.
	for _, pkg := range s.speaks {
		listed := false
		for _, status := range catalogue.Services {
			if status.Package == pkg {
				listed = true
				break
			}
		}
		if !listed {
			catalogue.Missing = append(catalogue.Missing, pkg)
		}
	}
	return catalogue
}

// packagesSpoken is every proto package this SDK speaks: `facade.ProtoPackages`
// filtered to the `loams.` ones, which is what `GetInstance.ApiVersions` names.
//
// `google.protobuf` is the well-known types, and a client never asks an
// instance to serve them, so including it would make every instance look
// incompatible.
func packagesSpoken() []string {
	var out []string
	for _, pkg := range facade.ProtoPackages {
		if strings.HasPrefix(pkg, "loams.") {
			out = append(out, pkg)
		}
	}
	return out
}

// Available reports whether this instance serves a proto package. It costs no
// RPC once the catalogue is cached.
//
// `loams.System().Available(ctx, "loams.live.v1")` is the question; a caller who
// thinks in module names uses `AvailableModule`.
func (s *System) Available(ctx context.Context, pkg string) (bool, error) {
	catalogue, err := s.Catalogue(ctx)
	if err != nil {
		return false, err
	}
	return catalogue.available(pkg), nil
}

// available is the catalogue-side half, with three states collapsed into a
// boolean: a package that is not listed at all is *not* available, which is the
// answer a caller wants even though it means something different.
func (c *Catalogue) available(pkg string) bool {
	for _, status := range c.Services {
		if status.Package == pkg {
			return status.Available
		}
	}
	return false
}

// Served are the packages this instance serves.
func (s *System) Served(ctx context.Context) ([]string, error) {
	catalogue, err := s.Catalogue(ctx)
	if err != nil {
		return nil, err
	}
	return append([]string(nil), catalogue.Served...), nil
}

// Unavailable are the packages this instance knows about and does not serve.
func (s *System) Unavailable(ctx context.Context) ([]string, error) {
	catalogue, err := s.Catalogue(ctx)
	if err != nil {
		return nil, err
	}
	return append([]string(nil), catalogue.Unavailable...), nil
}

// AvailableModule reports whether an SDK module's package is served, taking a
// **module name or a proto package** so a caller holding `loams.live` and a
// caller reading a `ServiceStatus` can both ask. A module name is resolved
// through the generated table, so `loams.live` and `loams.tables` — two facade
// names for one package — answer the same thing.
func (s *System) AvailableModule(ctx context.Context, moduleOrPackage string) (bool, error) {
	pkg, err := packageOf(moduleOrPackage)
	if err != nil {
		return false, err
	}
	return s.Available(ctx, pkg)
}

// packageOf is the proto package behind a module name or a package name.
func packageOf(moduleOrPackage string) (string, error) {
	if strings.HasPrefix(moduleOrPackage, "loams.") {
		return moduleOrPackage, nil
	}
	entry, ok := facade.Module(moduleOrPackage)
	if !ok {
		return "", newInternalError("", "loams has no generated module %s", moduleOrPackage)
	}
	return entry.Package, nil
}

// Guard returns a `FeatureNotInVariantError` when a module's package is not
// served here, and nil when it is.
//
// The error is the **same type** a refused RPC produces, so one `errors.As`
// covers both, and it costs no request once the catalogue is cached:
//
//	if err := client.System().Guard(ctx, "live"); err != nil {
//	    var absent *loams.FeatureNotInVariantError
//	    if errors.As(err, &absent) {
//	        return runWithoutLiveSync()
//	    }
//	}
//
// The message names the package rather than the module, because the package is
// what the server knows about and what an operator will grep for.
func (s *System) Guard(ctx context.Context, moduleOrPackage string) error {
	pkg, err := packageOf(moduleOrPackage)
	if err != nil {
		return err
	}
	available, err := s.Available(ctx, pkg)
	if err != nil {
		return err
	}
	if available {
		return nil
	}
	// The guard learned this from the catalogue rather than from a refusal, so
	// the `rpc` is `GetInstance` — the call that actually answered — and the
	// variant is recorded as unknown rather than invented. A plausible-looking
	// variant in a support ticket is worse than an honest "unknown".
	return &FeatureNotInVariantError{
		UnimplementedError: UnimplementedError{LoamsError{
			Code:     CodeUnimplemented,
			Reason:   ReasonFeatureNotInVariant,
			RPC:      instanceGetInstanceRPC,
			Hint:     "this build variant does not carry " + pkg,
			Metadata: map[string]string{"package": pkg, "variant": variantUnknown},
			Cause:    fmt.Errorf("loams.%s is not available on this instance: %s is not served", moduleOrPackage, pkg),
		}},
		Variant: variantUnknown,
	}
}

// variantUnknown is what `Variant` says when the SDK learned the package is
// absent from the catalogue rather than from a refusal.
const variantUnknown = "unknown"

// Version reports what this SDK speaks beside what the server serves (R9).
//
// A package the SDK speaks and the server does not serve is reported in
// `Missing` and `Compatible` is false; it is **not** an error. The SDK still
// works for the modules that are there, and what a missing one means is the
// caller's decision.
func (s *System) Version(ctx context.Context) (*VersionReport, error) {
	info, err := s.client.Instance().GetInstance(ctx, &GetInstanceRequest{})
	if err != nil {
		return nil, err
	}
	report := &VersionReport{
		ProtoRev:      facade.ProtoRev,
		ServerVersion: info.GetServerVersion(),
		APIVersions:   append([]string(nil), info.GetApiVersions()...),
		Compatible:    true,
	}
	for _, pkg := range s.speaks {
		served := false
		for _, servedPkg := range report.APIVersions {
			if servedPkg == pkg {
				served = true
				break
			}
		}
		if !served {
			report.Compatible = false
			report.Missing = append(report.Missing, pkg)
		}
	}
	return report, nil
}
