output "public_ip" {
  value = oci_core_instance.vm.public_ip
}

output "ssh" {
  value = "ssh -i ~/.ssh/kaveon-demo ubuntu@${oci_core_instance.vm.public_ip}"
}

output "image" {
  value = data.oci_core_images.ubuntu.images[0].display_name
}
