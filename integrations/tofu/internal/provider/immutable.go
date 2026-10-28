package provider

import (
	"context"
	"fmt"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/tfsdk"
	"github.com/hashicorp/terraform-plugin-framework/types"
)

// A meter's or a feature's semantics are immutable once it is created
// (§63 §17.1, §22): changing them in place would re-rate history. Replacing
// the resource is no way out either, because the server keeps an archived
// slug and refuses to reuse it. So a change to such an attribute is a plan
// error that asks for a new key, unless the key itself changes in the same
// plan (then the resource is replaced under the new key).

// refuseChange decides: the attribute had a value, the plan knows the new
// one, it differs, and the key stays.
func refuseChange(hadState, planKnown, equal, keyChanged bool) bool {
	return hadState && planKnown && !equal && !keyChanged
}

func keyChanged(ctx context.Context, key string, plan tfsdk.Plan, state tfsdk.State) bool {
	var p, s types.String
	if plan.GetAttribute(ctx, path.Root(key), &p).HasError() || state.GetAttribute(ctx, path.Root(key), &s).HasError() {
		return false
	}
	return !p.IsUnknown() && !p.Equal(s)
}

func immutableMessage(key, what string) string {
	return fmt.Sprintf("%s is part of this %s's semantics, which cannot change once created: "+
		"usage already recorded under it would be re-rated. Create a new %s with a new %s instead "+
		"(design §63 §22).", "this attribute", what, what, key)
}

// immutableString refuses a change of a string attribute unless `key` changes.
func immutableString(key, what string) planmodifier.String {
	return immutableStringModifier{key: key, what: what}
}

type immutableStringModifier struct{ key, what string }

func (m immutableStringModifier) Description(context.Context) string {
	return "Cannot change once created; use a new " + m.key + "."
}

func (m immutableStringModifier) MarkdownDescription(ctx context.Context) string {
	return m.Description(ctx)
}

func (m immutableStringModifier) PlanModifyString(ctx context.Context, req planmodifier.StringRequest, resp *planmodifier.StringResponse) {
	if req.State.Raw.IsNull() || req.Plan.Raw.IsNull() {
		return
	}
	if refuseChange(true, !req.PlanValue.IsUnknown(), req.PlanValue.Equal(req.StateValue),
		keyChanged(ctx, m.key, req.Plan, req.State)) {
		resp.Diagnostics.AddAttributeError(req.Path, "Immutable "+m.what+" attribute", immutableMessage(m.key, m.what))
	}
}

// immutableMap is immutableString for map attributes.
func immutableMap(key, what string) planmodifier.Map {
	return immutableMapModifier{key: key, what: what}
}

type immutableMapModifier struct{ key, what string }

func (m immutableMapModifier) Description(context.Context) string {
	return "Cannot change once created; use a new " + m.key + "."
}

func (m immutableMapModifier) MarkdownDescription(ctx context.Context) string {
	return m.Description(ctx)
}

func (m immutableMapModifier) PlanModifyMap(ctx context.Context, req planmodifier.MapRequest, resp *planmodifier.MapResponse) {
	if req.State.Raw.IsNull() || req.Plan.Raw.IsNull() {
		return
	}
	if refuseChange(true, !req.PlanValue.IsUnknown(), req.PlanValue.Equal(req.StateValue),
		keyChanged(ctx, m.key, req.Plan, req.State)) {
		resp.Diagnostics.AddAttributeError(req.Path, "Immutable "+m.what+" attribute", immutableMessage(m.key, m.what))
	}
}
