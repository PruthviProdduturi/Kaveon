variable "tenancy_ocid" {
  type        = string
  description = "Tenancy OCID (also the root compartment)."
}

variable "compartment_ocid" {
  type        = string
  description = "Compartment for every resource. The tenancy OCID is acceptable for a personal tenancy."
}

variable "region" {
  type        = string
  description = "Home region identifier, e.g. us-sanjose-1. Always Free A1 capacity varies by region."
}

variable "config_profile" {
  type        = string
  default     = "DEFAULT"
  description = "Profile in ~/.oci/config holding the API key."
}

variable "availability_domain_index" {
  type        = number
  default     = 0
  description = "Index into the region's availability domains; move if A1 capacity is exhausted in one."
}

variable "ssh_public_key" {
  type        = string
  description = "OpenSSH public key for the ubuntu administrator."
}

variable "operator_cidr" {
  type        = string
  description = "CIDR allowed to reach SSH."
}

variable "ocpus" {
  type    = number
  default = 4
}

variable "memory_gb" {
  type    = number
  default = 24
}

variable "boot_volume_gb" {
  type        = number
  default     = 150
  description = "Always Free covers 200 GB of block storage in total; leave headroom for backups."
}
