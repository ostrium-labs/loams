// Command quickstart is the smallest useful Loams client: read the endpoint and
// the key from the environment, ask the instance what it is, and print which
// packages it serves.
//
//	export LOAMS_ENDPOINT=http://127.0.0.1:8080
//	export LOAMS_API_KEY=…            # a loopback stack needs none
//	go run ./examples/quickstart
//
// There are no credentials in this file and none in the repository. The key is
// read from `LOAMS_API_KEY` because a key in a source file is a key in a git
// history (design §44 §7.4: the bearer travels in `Authorization`, never in a
// URL, and the examples say so by having no key to leak).
package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"time"

	"loams.dev/go"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "quickstart:", err)
		os.Exit(1)
	}
}

func run() error {
	endpoint := os.Getenv("LOAMS_ENDPOINT")
	if endpoint == "" {
		return errors.New("LOAMS_ENDPOINT is not set; it is the instance's base URL, for example http://127.0.0.1:8080")
	}

	client, err := loams.New(loams.Options{
		Endpoint: endpoint,
		// An API key is what a script has. A loopback `loams dev` has no
		// authentication, so this is harmless there; omit `Auth` entirely for one.
		Auth: loams.APIKey(os.Getenv("LOAMS_API_KEY")),
	})
	if err != nil {
		return err
	}
	defer client.Close()

	// The context is the deadline: one bound covers the call and every retry
	// inside it. There is no per-call timeout option in this SDK, because in Go
	// a deadline is not an option.
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	info, err := client.Instance().GetInstance(ctx, &loams.GetInstanceRequest{})
	if err != nil {
		// A reason is a stable string; a message is for people and may change.
		var absent *loams.FeatureNotInVariantError
		if errors.As(err, &absent) {
			return fmt.Errorf("this instance does not serve the package: %w", err)
		}
		return fmt.Errorf("asking the instance what it is: %w", err)
	}

	fmt.Printf("instance   %s (%s %s)\n", info.GetName(), info.GetEdition(), info.GetServerVersion())
	fmt.Printf("instanceId %s\n", info.GetInstanceId())
	fmt.Printf("api        %v\n", info.GetApiVersions())
	fmt.Printf("this SDK   proto revision %s\n", client.ProtoRev())

	// One call answers "what can this instance do", so a client never has to
	// discover it by calling something that will be refused.
	catalogue, err := client.System().Catalogue(ctx)
	if err != nil {
		return fmt.Errorf("reading the service catalogue: %w", err)
	}
	for _, served := range catalogue.Served {
		fmt.Printf("  serves     %s\n", served)
	}
	for _, missing := range catalogue.Unavailable {
		fmt.Printf("  unavailable %s\n", missing)
	}

	// A warning, not an error: the SDK still works for the packages that are
	// there, and what a missing one means is the caller's decision.
	report, err := client.System().Version(ctx)
	if err != nil {
		return fmt.Errorf("checking the proto revision: %w", err)
	}
	if !report.Compatible {
		fmt.Printf("  not served here: %v (this SDK speaks proto revision %s)\n", report.Missing, report.ProtoRev)
	}

	// The guard costs no RPC once the catalogue is cached, and raises the same
	// error type a refused call would — so one `errors.As` covers both.
	if err := client.System().Guard(ctx, "live"); err != nil {
		var notHere *loams.FeatureNotInVariantError
		if errors.As(err, &notHere) {
			fmt.Printf("  live sync   unavailable (variant %q)\n", notHere.Variant)
			return nil
		}
		return err
	}
	fmt.Println("  live sync   available")
	return nil
}
