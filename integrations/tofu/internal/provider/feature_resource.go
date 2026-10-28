package provider

import (
	"context"

	"github.com/hashicorp/terraform-plugin-framework/diag"
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
	_ resource.Resource                = (*featureResource)(nil)
	_ resource.ResourceWithConfigure   = (*featureResource)(nil)
	_ resource.ResourceWithImportState = (*featureResource)(nil)
)

// Units are §63 §9.2's named units.
var units = []string{"count", "ms", "cu_ms", "bytes", "byte_hours", "tokens", "requests"}

// NewFeatureResource is `loams_feature`.
func NewFeatureResource() resource.Resource { return &featureResource{} }

type featureResource struct{ c *client.Client }

type featureModel struct {
	Key    types.String `tfsdk:"key"`
	Name   types.String `tfsdk:"name"`
	Meter  types.String `tfsdk:"meter"`
	Filter types.Map    `tfsdk:"filter"`
	Unit   types.String `tfsdk:"unit"`
}

func (r *featureResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_feature"
}

func (r *featureResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	immutable := []planmodifier.String{immutableString("key", "feature")}
	resp.Schema = schema.Schema{
		Description: "A feature: what a customer is entitled to, optionally backed by a meter (design §63 §9). " +
			"Its meter, filter and unit cannot change once created; destroying it archives it.",
		Attributes: map[string]schema.Attribute{
			"key": schema.StringAttribute{
				Required:      true,
				Description:   "The feature's key, for example house_cpu (the slug rule).",
				PlanModifiers: []planmodifier.String{stringplanmodifier.RequiresReplace()},
				Validators:    []validator.String{matches(slugRe, 64, "a key: lower-case letters and digits in _-separated words")},
			},
			"name": schema.StringAttribute{Optional: true, Description: "A display name; can change."},
			"meter": schema.StringAttribute{
				Optional:      true,
				Description:   "The slug of the meter behind a metered feature; none for boolean and static features.",
				PlanModifiers: immutable,
			},
			"filter": schema.MapAttribute{
				Optional:      true,
				ElementType:   types.StringType,
				Description:   "Group-by values that select part of the meter, for example { model = \"m-large\" }.",
				PlanModifiers: []planmodifier.Map{immutableMap("key", "feature")},
			},
			"unit": schema.StringAttribute{
				Required:      true,
				Description:   "count, ms, cu_ms, bytes, byte_hours, tokens or requests.",
				PlanModifiers: immutable,
				Validators:    []validator.String{oneOf(units...)},
			},
		},
	}
}

func (r *featureResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	r.c = configureClient(req, resp)
}

func (m *featureModel) toAPI(ctx context.Context) (client.Feature, diag.Diagnostics) {
	out := client.Feature{
		Key:       m.Key.ValueString(),
		Name:      m.Name.ValueString(),
		MeterSlug: m.Meter.ValueString(),
		Unit:      m.Unit.ValueString(),
	}
	var diags diag.Diagnostics
	if !m.Filter.IsNull() && !m.Filter.IsUnknown() {
		out.Filter = map[string]string{}
		diags = m.Filter.ElementsAs(ctx, &out.Filter, false)
	}
	return out, diags
}

func (m *featureModel) fromAPI(ctx context.Context, a *client.Feature) diag.Diagnostics {
	m.Key = types.StringValue(a.Key)
	m.Name = optString(m.Name, a.Name)
	m.Meter = optString(m.Meter, a.MeterSlug)
	m.Unit = types.StringValue(a.Unit)
	if len(a.Filter) == 0 && (m.Filter.IsNull() || len(m.Filter.Elements()) == 0) {
		return nil
	}
	v, diags := types.MapValueFrom(ctx, types.StringType, a.Filter)
	m.Filter = v
	return diags
}

func (r *featureResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan featureModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	in, diags := plan.toAPI(ctx)
	resp.Diagnostics.Append(diags...)
	if resp.Diagnostics.HasError() {
		return
	}
	out, err := r.c.CreateFeature(ctx, in)
	if err != nil {
		resp.Diagnostics.AddError("Creating feature "+in.Key, err.Error())
		return
	}
	resp.Diagnostics.Append(plan.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *featureResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state featureModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	out, err := r.c.GetFeature(ctx, state.Key.ValueString())
	if client.IsNotFound(err) || (err == nil && out.Archived) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Reading feature "+state.Key.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(state.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *featureResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan featureModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	out, err := r.c.UpdateFeature(ctx, plan.Key.ValueString(), plan.Name.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Updating feature "+plan.Key.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(plan.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *featureResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state featureModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.c.ArchiveFeature(ctx, state.Key.ValueString()); err != nil && !client.IsNotFound(err) {
		resp.Diagnostics.AddError("Archiving feature "+state.Key.ValueString(), err.Error())
	}
}

func (r *featureResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("key"), req, resp)
}
