# Kaveon public demo on Oracle Cloud Always Free.
#
# One VM.Standard.A1.Flex instance (4 OCPU, 24 GB — the whole Always Free
# allowance) runs the Compose stack: KaveonDB coordinator and two workers, the
# API, PostgreSQL metadata, and Caddy as the only public door. The lake lives on
# the boot volume for now; object storage follows the Engine's S3 work.
#
# Apply with an API-key profile in ~/.oci/config:
#   terraform -chdir=infra/oci init
#   terraform -chdir=infra/oci apply -var-file=demo.tfvars

terraform {
  required_version = ">= 1.6"
  required_providers {
    oci = { source = "oracle/oci", version = "~> 6.0" }
  }
}

provider "oci" {
  region              = var.region
  config_file_profile = var.config_profile
}

locals {
  name = "kaveon-demo"
  tags = { project = "kaveon", environment = "demo", owner = "prproddu" }
}

data "oci_identity_availability_domains" "ads" {
  compartment_id = var.tenancy_ocid
}

# Newest Canonical Ubuntu 24.04 image for the A1 (aarch64) shape.
data "oci_core_images" "ubuntu" {
  compartment_id           = var.compartment_ocid
  operating_system         = "Canonical Ubuntu"
  operating_system_version = "24.04"
  shape                    = "VM.Standard.A1.Flex"
  sort_by                  = "TIMECREATED"
  sort_order               = "DESC"
}

# ── Network ──────────────────────────────────────────────────────────────────
resource "oci_core_vcn" "vcn" {
  compartment_id = var.compartment_ocid
  display_name   = "${local.name}-vcn"
  cidr_blocks    = ["10.70.0.0/24"]
  dns_label      = "kaveondemo"
  freeform_tags  = local.tags
}

resource "oci_core_internet_gateway" "igw" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.vcn.id
  display_name   = "${local.name}-igw"
  freeform_tags  = local.tags
}

resource "oci_core_route_table" "public" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.vcn.id
  display_name   = "${local.name}-rt"
  freeform_tags  = local.tags
  route_rules {
    destination       = "0.0.0.0/0"
    destination_type  = "CIDR_BLOCK"
    network_entity_id = oci_core_internet_gateway.igw.id
  }
}

# SSH from the operator only; HTTPS from anywhere; HTTP only for the certificate challenge.
resource "oci_core_security_list" "public" {
  compartment_id = var.compartment_ocid
  vcn_id         = oci_core_vcn.vcn.id
  display_name   = "${local.name}-sl"
  freeform_tags  = local.tags

  egress_security_rules {
    destination = "0.0.0.0/0"
    protocol    = "all"
  }
  ingress_security_rules {
    protocol = "6"
    source   = var.operator_cidr
    tcp_options { min = 22  max = 22 }
  }
  ingress_security_rules {
    protocol = "6"
    source   = "0.0.0.0/0"
    tcp_options { min = 443 max = 443 }
  }
  ingress_security_rules {
    protocol = "6"
    source   = "0.0.0.0/0"
    tcp_options { min = 80  max = 80 }
  }
}

resource "oci_core_subnet" "public" {
  compartment_id    = var.compartment_ocid
  vcn_id            = oci_core_vcn.vcn.id
  display_name      = "${local.name}-subnet"
  cidr_block        = "10.70.0.0/26"
  dns_label         = "vm"
  route_table_id    = oci_core_route_table.public.id
  security_list_ids = [oci_core_security_list.public.id]
  freeform_tags     = local.tags
}

# ── Instance ─────────────────────────────────────────────────────────────────
resource "oci_core_instance" "vm" {
  compartment_id      = var.compartment_ocid
  availability_domain = data.oci_identity_availability_domains.ads.availability_domains[var.availability_domain_index].name
  display_name        = local.name
  shape               = "VM.Standard.A1.Flex"
  freeform_tags       = local.tags

  shape_config {
    ocpus         = var.ocpus
    memory_in_gbs = var.memory_gb
  }

  source_details {
    source_type             = "image"
    source_id               = data.oci_core_images.ubuntu.images[0].id
    boot_volume_size_in_gbs = var.boot_volume_gb
  }

  create_vnic_details {
    subnet_id        = oci_core_subnet.public.id
    assign_public_ip = true
    hostname_label   = "kaveon"
  }

  metadata = {
    ssh_authorized_keys = var.ssh_public_key
    user_data           = base64encode(file("${path.module}/../bicep/cloud-init/demo-vm.yaml"))
  }

  preserve_boot_volume = true

  lifecycle {
    ignore_changes = [source_details[0].source_id] # keep the instance when a newer image appears
  }
}
