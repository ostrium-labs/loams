# terraform-provider-loams: Loams Meter as code

Status: **preview** (plan ME1 Task 33, design [§63 §22](../../docs/design/63-loams-meter.md), D1137, D2216). This provider needs the meter role's REST API (`/v1/meter/…`, ME-svc). Until that API is on `dev`, only the unit tests against a fake run.

An OpenTofu provider for Loams Meter's **definitions**: meters, features, plans and plan assignments. It targets OpenTofu's registry (`registry.opentofu.org/ostrium-labs/loams`) and plugin protocol 6. Terraform speaks the same protocol, but it is not tested or documented as a target, because its licence is BUSL 1.1. The provider never meters and never reads usage. Plans can also come from YAML in the GitOps repository (§22). Use one way per plan, not both.

| Resource | API | Notes |
|---|---|---|
| `loams_meter` | `MeterService` | `event_type`, `aggregation`, `value_property`, `group_by` and `event_from` cannot change once created. A change is a plan error that asks for a new `slug`. Destroy archives the meter, and its slug is never reused |
| `loams_feature` | features | `meter`, `filter` and `unit` cannot change once created. Destroy archives the feature |
| `loams_plan` | `PlanService.ApplyPlan` | `definition` is the plan's YAML. Every change needs a higher `version`, because published versions never change. Destroy removes the plan from state only |
| `loams_customer_plan` | `PlanService.AssignPlan` / `UnassignPlan` | Without `version`, the plan follows the latest version |

```hcl
terraform {
  required_providers {
    loams = { source = "ostrium-labs/loams" }
  }
}

provider "loams" {
  endpoint = "https://loams.example.ts.net" # or LOAMS_ENDPOINT
  # api_key from LOAMS_API_KEY: a key whose principal holds meter_admin
}

resource "loams_meter" "house_cpu_ms" {
  slug           = "house_cpu_ms"
  event_type     = "io.loams.dev.usage.house.query.v1"
  aggregation    = "SUM"
  value_property = "$.cpu_ms"
}

resource "loams_feature" "house_cpu" {
  key   = "house_cpu"
  meter = loams_meter.house_cpu_ms.slug
  unit  = "ms"
}

resource "loams_plan" "free" {
  key     = "free"
  version = 3
  definition = yamlencode({
    entitlements = [{
      feature = loams_feature.house_cpu.key
      metered = { period = { interval = "month" }, mode = "hard_limit", issue_after_reset = 36000000 }
    }]
  })
}

resource "loams_customer_plan" "org_42" {
  customer = "org_42"
  plan     = loams_plan.free.key
}
```

Payment state, spend controls and grants are not resources here. Payment state belongs to the billing exporter's service identity, spend controls to each customer's billing admins, and grants to `meter_admin` operators through the API, where each change is audited (§63 §19).

Development: `go test ./...`, `go vet ./...`. CI is `.github/workflows/tofu-provider.yml`.
