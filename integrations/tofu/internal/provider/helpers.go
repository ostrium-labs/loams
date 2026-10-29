package provider

import (
	"context"
	"fmt"
	"regexp"
	"slices"
	"strings"

	"github.com/hashicorp/terraform-plugin-framework/diag"
	"github.com/hashicorp/terraform-plugin-framework/schema/validator"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/ostrium-labs/loams/integrations/tofu/internal/client"
)

// ready reports whether the provider was configured; a resource used while
// the provider's own settings are unknown gets an error, not a panic.
func ready(c *client.Client, d *diag.Diagnostics) bool {
	if c == nil {
		d.AddError("Loams provider not configured",
			"endpoint and api_key are not known yet; apply what they depend on first")
		return false
	}
	return true
}

// optString keeps a null optional attribute null when the server answers
// with the empty string.
func optString(prior types.String, got string) types.String {
	if got == "" && prior.IsNull() {
		return types.StringNull()
	}
	return types.StringValue(got)
}

type oneOfValidator struct{ values []string }

func oneOf(values ...string) validator.String { return oneOfValidator{values: values} }

func (v oneOfValidator) Description(context.Context) string {
	return "one of " + strings.Join(v.values, ", ")
}

func (v oneOfValidator) MarkdownDescription(ctx context.Context) string { return v.Description(ctx) }

func (v oneOfValidator) ValidateString(_ context.Context, req validator.StringRequest, resp *validator.StringResponse) {
	if req.ConfigValue.IsNull() || req.ConfigValue.IsUnknown() {
		return
	}
	if !slices.Contains(v.values, req.ConfigValue.ValueString()) {
		resp.Diagnostics.AddAttributeError(req.Path, "Invalid value",
			fmt.Sprintf("%q is not %s", req.ConfigValue.ValueString(), v.Description(context.Background())))
	}
}

type matchValidator struct {
	re    *regexp.Regexp
	limit int
	what  string
}

func matches(re *regexp.Regexp, limit int, what string) validator.String {
	return matchValidator{re: re, limit: limit, what: what}
}

func (v matchValidator) Description(context.Context) string {
	return fmt.Sprintf("%s, at most %d characters", v.what, v.limit)
}

func (v matchValidator) MarkdownDescription(ctx context.Context) string { return v.Description(ctx) }

func (v matchValidator) ValidateString(_ context.Context, req validator.StringRequest, resp *validator.StringResponse) {
	if req.ConfigValue.IsNull() || req.ConfigValue.IsUnknown() {
		return
	}
	s := req.ConfigValue.ValueString()
	if len(s) > v.limit || !v.re.MatchString(s) {
		resp.Diagnostics.AddAttributeError(req.Path, "Invalid value",
			fmt.Sprintf("%q is not %s", s, v.Description(context.Background())))
	}
}
