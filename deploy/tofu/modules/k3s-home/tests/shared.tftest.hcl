# The shared module on its own: the default template must not change, and the
# k3s placeholders it takes must be safe to write into a file the VM sources.

run "vm_modules_render_unchanged" {
  module {
    source = "../shared"
  }

  command = plan

  variables {
    cloud                  = "aws"
    release                = "0.1.4"
    bucket                 = "loams-t-123456789012"
    region                 = "us-east-1"
    secret_prefix          = "loams/t"
    hostname               = "loams"
    confluent_integrations = ["confluent", "events"]
  }

  # The same replacements, written out independently of the module's chain.
  assert {
    condition = output.user_data == replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(replace(
      file("${path.module}/../../../byoc/aws/cloud-init.yaml"),
      "@@VERSION@@", "0.1.4"),
      "@@BUCKET@@", "loams-t-123456789012"),
      "@@REGION@@", "us-east-1"),
      "@@PROJECT@@", ""),
      "@@CONTAINER@@", ""),
      "@@STORAGE_ACCOUNT@@", ""),
      "@@VAULT@@", ""),
      "@@CLIENT_ID@@", ""),
      "@@SECRET_PREFIX@@", "loams/t"),
      "@@PROFILES@@", ""),
      "@@HOSTNAME@@", "loams"),
      "@@DATA_DEVICE@@", ""),
    "@@CONFLUENT_INTEGRATIONS@@", "confluent,events")
    error_message = "the default template renders as it did before the k3s placeholders were added"
  }
  assert {
    condition     = !can(regex("loams-k3s-bootstrap", output.user_data))
    error_message = "the default template is the compose file"
  }
}

run "k3s_template_needs_its_inputs" {
  module {
    source = "../shared"
  }

  command = plan

  variables {
    cloud         = "aws"
    release       = "0.1.4"
    secret_prefix = "loams/t"
    template      = "cloud-init-k3s.yaml"
  }

  expect_failures = [output.user_data]
}

run "rejects_an_unknown_template" {
  module {
    source = "../shared"
  }

  command = plan

  variables {
    cloud         = "aws"
    release       = "0.1.4"
    secret_prefix = "loams/t"
    template      = "../../etc/passwd"
  }

  expect_failures = [var.template]
}

run "rejects_non_base64_values" {
  module {
    source = "../shared"
  }

  command = plan

  variables {
    cloud            = "aws"
    release          = "0.1.4"
    secret_prefix    = "loams/t"
    home_values_b64  = "abc$(reboot)"
    home_secrets_b64 = "ab c"
    target_facts_b64 = "a\nb"
  }

  expect_failures = [var.home_values_b64, var.home_secrets_b64, var.target_facts_b64]
}

run "rejects_an_unsafe_access_db_and_domain" {
  module {
    source = "../shared"
  }

  command = plan

  variables {
    cloud          = "aws"
    release        = "0.1.4"
    secret_prefix  = "loams/t"
    access_db      = "local; reboot"
    tailnet_domain = "a`b`.ts.net"
    deployment     = "x$y"
    chart_digest   = "sha256:abc"
  }

  expect_failures = [var.access_db, var.tailnet_domain, var.deployment, var.chart_digest]
}
