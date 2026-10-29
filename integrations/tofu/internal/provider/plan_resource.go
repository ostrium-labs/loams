package provider

import (
	"context"
	"fmt"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/schema/validator"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/ostrium-labs/loams/integrations/tofu/internal/client"
)

var (
	_ resource.Resource                = (*planResource)(nil)
	_ resource.ResourceWithConfigure   = (*planResource)(nil)
	_ resource.ResourceWithModifyPlan  = (*planResource)(nil)
	_ resource.ResourceWithImportState = (*planResource)(nil)
)

// NewPlanResource is `loams_plan`.
func NewPlanResource() resource.Resource { return &planResource{} }

type planResource struct{ c *client.Client }

type planModel struct {
	Key        types.String `tfsdk:"key"`
	Version    types.Int64  `tfsdk:"version"`
	Definition types.String `tfsdk:"definition"`
}

func (r *planResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_plan"
}

func (r *planResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		Description: "A plan: a versioned list of entitlement templates (design §63 §9.5), applied with " +
			"PlanService.ApplyPlan as the GitOps reconciler does (§22). Every change of the definition " +
			"is a new version; customers move to it at their next period. Destroying it removes it from " +
			"state only: published plan versions are kept.",
		Attributes: map[string]schema.Attribute{
			"key": schema.StringAttribute{
				Required:      true,
				Description:   "The plan's key, for example free.",
				PlanModifiers: []planmodifier.String{stringplanmodifier.RequiresReplace()},
				Validators:    []validator.String{matches(slugRe, 64, "a key: lower-case letters and digits in _-separated words")},
			},
			"version": schema.Int64Attribute{
				Required:    true,
				Description: "The version to publish. Raise it with every change of definition.",
			},
			"definition": schema.StringAttribute{
				Required: true,
				Description: "The plan's YAML (§63 §9.5): `entitlements:` with feature, metered/static/boolean. " +
					"Usually file(\"plans/free.yaml\") or yamlencode({...}).",
			},
		},
	}
}

func (r *planResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	r.c = configureClient(req, resp)
}

// versionError says why a plan change is refused, or "".
func versionError(stateVersion, planVersion int64, definitionChanged bool) string {
	switch {
	case planVersion < 1:
		return "version starts at 1"
	case planVersion < stateVersion:
		return fmt.Sprintf("version %d is older than the published %d: plan versions only move forward", planVersion, stateVersion)
	case definitionChanged && planVersion == stateVersion:
		return fmt.Sprintf("the definition changed but the version is still %d: raise version, "+
			"since a published plan version never changes", stateVersion)
	}
	return ""
}

func (r *planResource) ModifyPlan(ctx context.Context, req resource.ModifyPlanRequest, resp *resource.ModifyPlanResponse) {
	if req.Plan.Raw.IsNull() || req.State.Raw.IsNull() {
		return
	}
	var plan, state planModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || plan.Version.IsUnknown() || plan.Definition.IsUnknown() {
		return
	}
	if !plan.Key.Equal(state.Key) {
		return // replaced
	}
	if msg := versionError(state.Version.ValueInt64(), plan.Version.ValueInt64(), !plan.Definition.Equal(state.Definition)); msg != "" {
		resp.Diagnostics.AddAttributeError(path.Root("version"), "Plan version", msg)
	}
}

func (r *planResource) apply(ctx context.Context, m *planModel) error {
	out, err := r.c.ApplyPlan(ctx, client.Plan{
		Key:        m.Key.ValueString(),
		Version:    m.Version.ValueInt64(),
		Definition: m.Definition.ValueString(),
	})
	if err != nil {
		return err
	}
	if out.Version != 0 && out.Version != m.Version.ValueInt64() {
		return fmt.Errorf("the service published version %d, not %d", out.Version, m.Version.ValueInt64())
	}
	return nil
}

func (r *planResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan planModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if msg := versionError(0, plan.Version.ValueInt64(), false); msg != "" {
		resp.Diagnostics.AddAttributeError(path.Root("version"), "Plan version", msg)
		return
	}
	if err := r.apply(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Applying plan "+plan.Key.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *planResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state planModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	out, err := r.c.GetPlan(ctx, state.Key.ValueString())
	if client.IsNotFound(err) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Reading plan "+state.Key.ValueString(), err.Error())
		return
	}
	// A newer version published elsewhere (the GitOps repository) shows as
	// drift: the next apply publishes this configuration's version again
	// only if it is newer still, and the service refuses an older one.
	prior := state.Version.ValueInt64()
	if out.Definition != "" && (out.Version != prior || state.Definition.IsNull()) {
		state.Definition = types.StringValue(out.Definition)
	}
	state.Version = types.Int64Value(out.Version)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *planResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan planModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.apply(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Applying plan "+plan.Key.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *planResource) Delete(_ context.Context, _ resource.DeleteRequest, resp *resource.DeleteResponse) {
	resp.Diagnostics.AddWarning("Plan kept",
		"Published plan versions are never deleted (customers' entitlements refer to them); "+
			"the plan was removed from this state only. Unassign it from customers with loams_customer_plan.")
}

func (r *planResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("key"), req, resp)
}
