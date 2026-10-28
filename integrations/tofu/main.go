// Command terraform-provider-loams is the OpenTofu provider for Loams Meter's
// definitions: meters, features, plans and customer plan assignments (design
// §63 §22, D1137; plan ME1 Task 33; D2216).
//
// It targets OpenTofu's registry and plugin protocol 6. Terraform speaks the
// same protocol, but it is neither tested nor documented as a target, since
// its licence is BUSL 1.1 (§63 §22, D1833). The provider only provisions
// definitions; it never meters and never reads usage.
package main

import (
	"context"
	"flag"
	"log"

	"github.com/hashicorp/terraform-plugin-framework/providerserver"

	"github.com/ostrium-labs/loams/integrations/tofu/internal/provider"
)

// version is set by the release build (-ldflags "-X main.version=…").
var version = "dev"

func main() {
	var debug bool
	flag.BoolVar(&debug, "debug", false, "run with support for debuggers such as delve")
	flag.Parse()

	err := providerserver.Serve(context.Background(), provider.New(version), providerserver.ServeOpts{
		Address: "registry.opentofu.org/ostrium-labs/loams",
		Debug:   debug,
	})
	if err != nil {
		log.Fatal(err)
	}
}
