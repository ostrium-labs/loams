package provider

import (
	"context"
	"testing"

	"github.com/hashicorp/terraform-plugin-framework/providerserver"
	"github.com/hashicorp/terraform-plugin-go/tfprotov6"
)

func TestProviderSchemaIsValidAndListsTheResources(t *testing.T) {
	server, err := providerserver.NewProtocol6WithError(New("test")())()
	if err != nil {
		t.Fatal(err)
	}
	resp, err := server.GetProviderSchema(context.Background(), &tfprotov6.GetProviderSchemaRequest{})
	if err != nil {
		t.Fatal(err)
	}
	for _, d := range resp.Diagnostics {
		if d.Severity == tfprotov6.DiagnosticSeverityError {
			t.Errorf("schema: %s: %s", d.Summary, d.Detail)
		}
	}
	for _, name := range []string{"loams_meter", "loams_feature", "loams_plan", "loams_customer_plan"} {
		if _, ok := resp.ResourceSchemas[name]; !ok {
			t.Errorf("resource %s missing", name)
		}
	}
	key := resp.Provider.Block.Attributes
	for _, a := range key {
		if a.Name == "api_key" && !a.Sensitive {
			t.Error("api_key is not sensitive")
		}
	}
}

func TestMeterSemanticsCannotChangeInPlace(t *testing.T) {
	cases := []struct {
		name                                   string
		hadState, planKnown, equal, keyChanged bool
		refuse                                 bool
	}{
		{"create", false, true, false, false, false},
		{"unchanged", true, true, true, false, false},
		{"changed in place", true, true, false, false, true},
		{"changed with a new slug (replace)", true, true, false, true, false},
		{"unknown until apply", true, false, false, false, false},
	}
	for _, c := range cases {
		if got := refuseChange(c.hadState, c.planKnown, c.equal, c.keyChanged); got != c.refuse {
			t.Errorf("%s: refuse=%v, want %v", c.name, got, c.refuse)
		}
	}
}

func TestPlanVersionsOnlyMoveForward(t *testing.T) {
	cases := []struct {
		state, plan int64
		changed     bool
		ok          bool
	}{
		{3, 3, false, true},
		{3, 4, true, true},
		{3, 4, false, true},
		{3, 3, true, false},
		{3, 2, false, false},
		{0, 0, false, false},
	}
	for _, c := range cases {
		if got := versionError(c.state, c.plan, c.changed) == ""; got != c.ok {
			t.Errorf("state %d, plan %d, changed %v: ok=%v, want %v", c.state, c.plan, c.changed, got, c.ok)
		}
	}
}

func TestValidators(t *testing.T) {
	if !slugRe.MatchString("house_cpu_ms") || slugRe.MatchString("House-CPU") || slugRe.MatchString("a__b") {
		t.Error("slug rule")
	}
}
