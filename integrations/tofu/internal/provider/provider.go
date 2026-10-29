// Package provider is the OpenTofu provider for Loams Meter's definitions
// (design §63 §22, D1137; plan ME1 Task 33; D2216).
package provider

import (
	"context"
	"os"

	"github.com/hashicorp/terraform-plugin-framework/datasource"
	"github.com/hashicorp/terraform-plugin-framework/provider"
	"github.com/hashicorp/terraform-plugin-framework/provider/schema"
	"github.com/hashicorp/terraform-plugin-framework/resource"
	"github.com/hashicorp/terraform-plugin-framework/types"

	"github.com/ostrium-labs/loams/integrations/tofu/internal/client"
)

var _ provider.Provider = (*loamsProvider)(nil)

type loamsProvider struct {
	version string
}

// New returns the provider's constructor.
func New(version string) func() provider.Provider {
	return func() provider.Provider { return &loamsProvider{version: version} }
}

type providerModel struct {
	Endpoint types.String `tfsdk:"endpoint"`
	APIKey   types.String `tfsdk:"api_key"`
}

func (p *loamsProvider) Metadata(_ context.Context, _ provider.MetadataRequest, resp *provider.MetadataResponse) {
	resp.TypeName = "loams"
	resp.Version = p.version
}

func (p *loamsProvider) Schema(_ context.Context, _ provider.SchemaRequest, resp *provider.SchemaResponse) {
	resp.Schema = schema.Schema{
		Description: "Loams Meter's meters, features, plans and plan assignments, as code (design §63 §22). " +
			"It provisions definitions only; it never meters.",
		Attributes: map[string]schema.Attribute{
			"endpoint": schema.StringAttribute{
				Optional:    true,
				Description: "The Loams deployment's API URL, for example https://loams.example.ts.net. Default: LOAMS_ENDPOINT.",
			},
			"api_key": schema.StringAttribute{
				Optional:    true,
				Sensitive:   true,
				Description: "A Loams API key whose principal holds meter_admin. Default: LOAMS_API_KEY.",
			},
		},
	}
}

func (p *loamsProvider) Configure(ctx context.Context, req provider.ConfigureRequest, resp *provider.ConfigureResponse) {
	var cfg providerModel
	resp.Diagnostics.Append(req.Config.Get(ctx, &cfg)...)
	if resp.Diagnostics.HasError() {
		return
	}
	if cfg.Endpoint.IsUnknown() || cfg.APIKey.IsUnknown() {
		// Known only after an apply elsewhere; resources stay unconfigured
		// during this plan.
		return
	}
	endpoint := cfg.Endpoint.ValueString()
	if endpoint == "" {
		endpoint = os.Getenv("LOAMS_ENDPOINT")
	}
	key := cfg.APIKey.ValueString()
	if key == "" {
		key = os.Getenv("LOAMS_API_KEY")
	}
	c, err := client.New(endpoint, key, "terraform-provider-loams/"+p.version)
	if err != nil {
		resp.Diagnostics.AddError("Loams provider configuration",
			err.Error()+": set endpoint and api_key, or LOAMS_ENDPOINT and LOAMS_API_KEY")
		return
	}
	resp.ResourceData = c
	resp.DataSourceData = c
}

func (p *loamsProvider) Resources(_ context.Context) []func() resource.Resource {
	return []func() resource.Resource{
		NewMeterResource,
		NewFeatureResource,
		NewPlanResource,
		NewCustomerPlanResource,
	}
}

func (p *loamsProvider) DataSources(_ context.Context) []func() datasource.DataSource {
	return nil
}

// configureClient takes the provider's client in a resource's Configure.
func configureClient(req resource.ConfigureRequest, resp *resource.ConfigureResponse) *client.Client {
	if req.ProviderData == nil {
		return nil
	}
	c, ok := req.ProviderData.(*client.Client)
	if !ok {
		resp.Diagnostics.AddError("Unexpected provider data", "the Loams provider was not configured with its client")
		return nil
	}
	return c
}
