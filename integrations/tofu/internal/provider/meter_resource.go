package provider

import (
	"context"
	"regexp"
	"strings"

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
	_ resource.Resource                = (*meterResource)(nil)
	_ resource.ResourceWithConfigure   = (*meterResource)(nil)
	_ resource.ResourceWithImportState = (*meterResource)(nil)
)

// slugRe is §63 §8.1's slug: ^[a-z0-9]+(?:_[a-z0-9]+)*$, at most 64.
var slugRe = regexp.MustCompile(`^[a-z0-9]+(?:_[a-z0-9]+)*$`)

// Aggregations are OpenMeter's.
var aggregations = []string{"SUM", "COUNT", "UNIQUE_COUNT", "AVG", "MIN", "MAX", "LATEST"}

// NewMeterResource is `loams_meter`.
func NewMeterResource() resource.Resource { return &meterResource{} }

type meterResource struct{ c *client.Client }

type meterModel struct {
	Slug          types.String `tfsdk:"slug"`
	Name          types.String `tfsdk:"name"`
	Description   types.String `tfsdk:"description"`
	EventType     types.String `tfsdk:"event_type"`
	Aggregation   types.String `tfsdk:"aggregation"`
	ValueProperty types.String `tfsdk:"value_property"`
	GroupBy       types.Map    `tfsdk:"group_by"`
	EventFrom     types.String `tfsdk:"event_from"`
}

func (r *meterResource) Metadata(_ context.Context, req resource.MetadataRequest, resp *resource.MetadataResponse) {
	resp.TypeName = req.ProviderTypeName + "_meter"
}

func (r *meterResource) Schema(_ context.Context, _ resource.SchemaRequest, resp *resource.SchemaResponse) {
	immutable := func() []planmodifier.String { return []planmodifier.String{immutableString("slug", "meter")} }
	resp.Schema = schema.Schema{
		Description: "A meter: turns usage events of one type into numbers (design §63 §8.1, OpenMeter's meter). " +
			"Its semantics cannot change once created; destroying it archives it, and its slug is never reused.",
		Attributes: map[string]schema.Attribute{
			"slug": schema.StringAttribute{
				Required:      true,
				Description:   "The meter's key: lower-case letters and digits in _-separated words, at most 64 characters.",
				PlanModifiers: []planmodifier.String{stringplanmodifier.RequiresReplace()},
				Validators:    []validator.String{matches(slugRe, 64, "a slug: lower-case letters and digits in _-separated words")},
			},
			"name":        schema.StringAttribute{Optional: true, Description: "A display name; can change."},
			"description": schema.StringAttribute{Optional: true, Description: "A description; can change."},
			"event_type": schema.StringAttribute{
				Required:      true,
				Description:   "The CloudEvents type the meter selects, for example io.loams.dev.usage.house.query.v1.",
				PlanModifiers: immutable(),
			},
			"aggregation": schema.StringAttribute{
				Required:      true,
				Description:   "SUM, COUNT, UNIQUE_COUNT, AVG, MIN, MAX or LATEST.",
				PlanModifiers: immutable(),
				Validators:    []validator.String{oneOf(aggregations...)},
			},
			"value_property": schema.StringAttribute{
				Optional:      true,
				Description:   "A JSONPath into the event's data, such as $.cpu_ms (not used by COUNT).",
				PlanModifiers: immutable(),
			},
			"group_by": schema.MapAttribute{
				Optional:      true,
				ElementType:   types.StringType,
				Description:   "Dimension name to JSONPath, at most 8.",
				PlanModifiers: []planmodifier.Map{immutableMap("slug", "meter")},
			},
			"event_from": schema.StringAttribute{
				Optional:      true,
				Description:   "RFC 3339: events before this time are ignored.",
				PlanModifiers: immutable(),
			},
		},
	}
}

func (r *meterResource) Configure(_ context.Context, req resource.ConfigureRequest, resp *resource.ConfigureResponse) {
	r.c = configureClient(req, resp)
}

func (m *meterModel) toAPI(ctx context.Context) (client.Meter, diag.Diagnostics) {
	out := client.Meter{
		Slug:          m.Slug.ValueString(),
		Name:          m.Name.ValueString(),
		Description:   m.Description.ValueString(),
		EventType:     m.EventType.ValueString(),
		Aggregation:   m.Aggregation.ValueString(),
		ValueProperty: m.ValueProperty.ValueString(),
		EventFrom:     m.EventFrom.ValueString(),
	}
	var diags diag.Diagnostics
	if !m.GroupBy.IsNull() && !m.GroupBy.IsUnknown() {
		out.GroupBy = map[string]string{}
		diags = m.GroupBy.ElementsAs(ctx, &out.GroupBy, false)
		if len(out.GroupBy) > 8 {
			diags.AddAttributeError(path.Root("group_by"), "Too many dimensions", "a meter has at most 8 group_by dimensions")
		}
	}
	return out, diags
}

// fromAPI copies what the server holds, keeping null for empty optional
// values the configuration left out.
func (m *meterModel) fromAPI(ctx context.Context, a *client.Meter) diag.Diagnostics {
	m.Slug = types.StringValue(a.Slug)
	m.Name = optString(m.Name, a.Name)
	m.Description = optString(m.Description, a.Description)
	m.EventType = types.StringValue(a.EventType)
	m.Aggregation = types.StringValue(strings.ToUpper(a.Aggregation))
	m.ValueProperty = optString(m.ValueProperty, a.ValueProperty)
	m.EventFrom = optString(m.EventFrom, a.EventFrom)
	if len(a.GroupBy) == 0 && (m.GroupBy.IsNull() || len(m.GroupBy.Elements()) == 0) {
		return nil
	}
	v, diags := types.MapValueFrom(ctx, types.StringType, a.GroupBy)
	m.GroupBy = v
	return diags
}

func (r *meterResource) Create(ctx context.Context, req resource.CreateRequest, resp *resource.CreateResponse) {
	var plan meterModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	in, diags := plan.toAPI(ctx)
	resp.Diagnostics.Append(diags...)
	if resp.Diagnostics.HasError() {
		return
	}
	out, err := r.c.CreateMeter(ctx, in)
	if err != nil {
		resp.Diagnostics.AddError("Creating meter "+in.Slug, err.Error())
		return
	}
	resp.Diagnostics.Append(plan.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *meterResource) Read(ctx context.Context, req resource.ReadRequest, resp *resource.ReadResponse) {
	var state meterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	out, err := r.c.GetMeter(ctx, state.Slug.ValueString())
	if client.IsNotFound(err) || (err == nil && out.Archived) {
		resp.State.RemoveResource(ctx)
		return
	}
	if err != nil {
		resp.Diagnostics.AddError("Reading meter "+state.Slug.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(state.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &state)...)
}

func (r *meterResource) Update(ctx context.Context, req resource.UpdateRequest, resp *resource.UpdateResponse) {
	var plan meterModel
	resp.Diagnostics.Append(req.Plan.Get(ctx, &plan)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	// Only the display fields reach here: the plan modifiers refuse the rest.
	out, err := r.c.UpdateMeter(ctx, plan.Slug.ValueString(), plan.Name.ValueString(), plan.Description.ValueString())
	if err != nil {
		resp.Diagnostics.AddError("Updating meter "+plan.Slug.ValueString(), err.Error())
		return
	}
	resp.Diagnostics.Append(plan.fromAPI(ctx, out)...)
	resp.Diagnostics.Append(resp.State.Set(ctx, &plan)...)
}

func (r *meterResource) Delete(ctx context.Context, req resource.DeleteRequest, resp *resource.DeleteResponse) {
	var state meterModel
	resp.Diagnostics.Append(req.State.Get(ctx, &state)...)
	if resp.Diagnostics.HasError() || !ready(r.c, &resp.Diagnostics) {
		return
	}
	if err := r.c.ArchiveMeter(ctx, state.Slug.ValueString()); err != nil && !client.IsNotFound(err) {
		resp.Diagnostics.AddError("Archiving meter "+state.Slug.ValueString(), err.Error())
	}
}

func (r *meterResource) ImportState(ctx context.Context, req resource.ImportStateRequest, resp *resource.ImportStateResponse) {
	resource.ImportStatePassthroughID(ctx, path.Root("slug"), req, resp)
}
