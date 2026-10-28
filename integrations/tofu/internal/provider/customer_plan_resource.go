package provider

import (
	"context"

	"github.com/hashicorp/terraform-plugin-framework/path"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/planmodifier"
	"github.com/hashicorp/terraform-plugin-framework/resource/schema/stringplanmodifier"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/ostrium-labs/loams/integrations/tofu/internal/client"
)

var (
	_ resource.Resource                = (*customerPlanResource)(nil)
	_ resource.ResourceWithConfigure   = (*customerPlanResource)(nil)
	_ resource.ResourceWithImportState = (*customerPlanResource)(nil)
)

// NewCustomerPlanResource is `loams_customer_plan`.
func NewCustomerPlanResource() resource.Resource { return &customerPlanResource{} }

type customerPlanResource struct{ c *client.Client }

type customerPlanModel struct {
	Customer types.String `tfsdk:"customer"`
	Plan     types.String `tfsdk:"plan"`
	Version  types.Int64  `tfsdk:"version"`
}

func (r *customerPlanResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_customer_plan"
}

func (r *customerPlanResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	resp.Schema = schema.Schema{
		Description: "Assigns a plan to a customer (PlanService.AssignPlan, design §63 §9.5): its entitlements " +
			"and recurring grants follow the plan; a change takes effect at the customer's next period. " +
			"Destroying it unassigns the plan. Payment state and spend controls are not set here: they " +
			"belong to the billing exporter and the customer's billing admins.",
		Attributes: map[string]schema.Attribute{
			"customer": schema.StringAttribute{
				Required:      true,
				Description:   "The customer's key (an org's customer for Loams' own billing).",
				PlanModifiers: []planmodifier.String{stringplanmodifier.RequiresReplace()},
			},
			"plan": schema.StringAttribute{
				Required:    true,
				Description: "The plan's key, usually loams_plan.<name>.key.",
			},
			"version": schema.Int64Attribute{
				Optional: true,
				Description: "The plan version. Omitted, the service assigns the latest at each apply " +
					"of this resource and the customer follows it at the next period.",
			},
		},
	}
}

func (r *customerPlanResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	r.c = configureClient(req, resp)
}

func (r *customerPlanResource) assign(ctx context.Context, m *customerPlanModel) error {
	ref := client.PlanRef{Plan: m.Plan.ValueString()}
	if !m.Version.IsNull() && !m.Version.IsUnknown() {
		ref.Version = m.Version.ValueInt64()
	}
	_, err := r.c.AssignPlan(ctx, m.Customer.ValueString(), ref)
	return err
}

func (r *customerPlanResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan customerPlanModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.assign(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Assigning plan "+plan.Plan.ValueString()+" to "+plan.Customer.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *customerPlanResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state customerPlanModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	out, err := r.c.GetCustomer(ctx, state.Customer.ValueString())
	if client.IsNotFound(err) || (err == nil && out.Plan == nil) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Reading customer "+state.Customer.ValueString(), err.Error())
		return
	}
	state.Plan = types.StringValue(out.Plan.Plan)
	// A configuration without a version follows the latest; only a pinned
	// version is compared.
	if !state.Version.IsNull() {
		state.Version = types.Int64Value(out.Plan.Version)
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *customerPlanResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan customerPlanModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.assign(ctx, &plan); err != nil {
		resp.Diagnostics.AddError("Assigning plan "+plan.Plan.ValueString()+" to "+plan.Customer.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *customerPlanResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state customerPlanModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.c.UnassignPlan(ctx, state.Customer.ValueString()); err != nil && !client.IsNotFound(err) {
		resp.Diagnostics.AddError("Unassigning the plan of "+state.Customer.ValueString(), err.Error())
	}
}

func (r *customerPlanResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("customer"), req, resp)
}
