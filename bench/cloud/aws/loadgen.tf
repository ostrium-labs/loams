# The in-region loadgen (plan §5.4): c7a.large on Amazon Linux 2023 in the
# default VPC, an outbound-only public IPv4 (no NAT), no inbound rule, IMDSv2
# only. Its role may change only the bench function's configuration (the cold
# scenario) and write under the run's prefix in the results bucket. It
# terminates itself at the deadline (shutdown behaviour terminate).

data "aws_ssm_parameter" "al2023" {
  name = "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"
}

data "aws_vpc" "default" {
  default = true
}

data "aws_ec2_instance_type_offerings" "loadgen" {
  location_type = "availability-zone"
  filter {
    name   = "instance-type"
    values = [var.loadgen_instance_type]
  }
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
  filter {
    name   = "default-for-az"
    values = ["true"]
  }
  filter {
    name   = "availability-zone"
    values = data.aws_ec2_instance_type_offerings.loadgen.locations
  }
}

resource "aws_security_group" "loadgen" {
  name        = "${local.name}-loadgen"
  description = "CB1 loadgen: no inbound rule, outbound only"
  vpc_id      = data.aws_vpc.default.id
  depends_on  = [module.app]
}

resource "aws_vpc_security_group_egress_rule" "loadgen_v4" {
  security_group_id = aws_security_group.loadgen.id
  ip_protocol       = "-1"
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_vpc_security_group_egress_rule" "loadgen_v6" {
  security_group_id = aws_security_group.loadgen.id
  ip_protocol       = "-1"
  cidr_ipv6         = "::/0"
}

data "aws_iam_policy_document" "assume_ec2" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ec2.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "loadgen" {
  name               = "${local.name}-loadgen"
  assume_role_policy = data.aws_iam_policy_document.assume_ec2.json
  depends_on         = [module.app]
}

data "aws_iam_policy_document" "loadgen" {
  statement {
    sid = "ColdStart"
    actions = [
      "lambda:GetFunction", "lambda:GetFunctionConfiguration", "lambda:UpdateFunctionConfiguration",
    ]
    resources = ["arn:aws:lambda:${var.region}:${local.account}:function:${module.app.function_name}"]
  }
  statement {
    sid       = "Results"
    actions   = ["s3:PutObject"]
    resources = ["${aws_s3_bucket.results.arn}/${local.prefix}*"]
  }
}

resource "aws_iam_role_policy" "loadgen" {
  name   = "loadgen"
  role   = aws_iam_role.loadgen.id
  policy = data.aws_iam_policy_document.loadgen.json
}

resource "aws_iam_instance_profile" "loadgen" {
  name = "${local.name}-loadgen"
  role = aws_iam_role.loadgen.name
}

locals {
  loadgen_files = {
    for f in ["run-scenarios.sh", "cold-aws.sh", "upload-aws.sh"] :
    f => base64gzip(file("${path.module}/../loadgen/${f}"))
  }
  loadgen_env = merge(var.bench_env, {
    BENCH_CLOUD         = "aws"
    BENCH_RUN_ID        = var.run_id
    BENCH_ROW           = var.row
    BENCH_URL           = module.app.url
    BENCH_REGION        = var.region
    AWS_DEFAULT_REGION  = var.region
    BENCH_INSTANCE_TYPE = var.loadgen_instance_type
    BENCH_COMMIT        = var.commit
    BENCH_FUNCTION      = module.app.function_name
    BENCH_BUCKET        = aws_s3_bucket.results.bucket
  })
}

resource "aws_instance" "loadgen" {
  ami                                  = data.aws_ssm_parameter.al2023.insecure_value
  instance_type                        = var.loadgen_instance_type
  subnet_id                            = sort(data.aws_subnets.default.ids)[0]
  vpc_security_group_ids               = [aws_security_group.loadgen.id]
  associate_public_ip_address          = true
  iam_instance_profile                 = aws_iam_instance_profile.loadgen.name
  instance_initiated_shutdown_behavior = "terminate"
  user_data_replace_on_change          = true

  user_data = templatefile("${path.module}/../loadgen/boot.sh.tftpl", {
    cloud       = "aws"
    max_minutes = var.max_minutes
    oha_url     = var.oha_url
    oha_sha256  = var.oha_sha256
    files       = local.loadgen_files
    env         = local.loadgen_env
  })

  metadata_options {
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  root_block_device {
    volume_type           = "gp3"
    volume_size           = 8
    encrypted             = true
    delete_on_termination = true
  }

  tags = { Name = "${local.name}-loadgen" }

  # The app (and its budget) exist before the loadgen starts measuring them.
  depends_on = [module.app, aws_iam_role_policy.loadgen, aws_s3_bucket_public_access_block.results]
}
