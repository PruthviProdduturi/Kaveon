// Kaveon public demo — one Spot VM running the Compose stack behind Caddy.
//
// Deploys beside the existing production resources in the personal subscription
// (registry and Key Vault are referenced, not created). The VM's system identity
// reads the lake and pulls images; nothing here stores a secret. Spot keeps the
// demo always on for a fraction of on-demand; an eviction deallocates rather than
// deletes, state lives on the managed disk and in ADLS, and the watchdog Logic
// App starts the VM again within its recurrence.
targetScope = 'resourceGroup'

param location string = resourceGroup().location
@description('Administrator login for SSH. Password authentication is disabled.')
param adminUsername string = 'kaveon'
@description('OpenSSH public key for the administrator.')
param sshPublicKey string
@description('CIDR allowed to reach SSH; everything else is denied.')
param operatorIpCidr string
param vmSize string = 'Standard_D4s_v5'
@description('Spot priority with deallocate-on-eviction. Set false only for a showcase day.')
param spot bool = true
@description('DNS label for the public IP: <label>.<region>.cloudapp.azure.com is the API host until a custom domain fronts it.')
param dnsLabel string = 'kaveon-demo'
@description('Existing container registry in this resource group.')
param registryName string = 'kaveonacr'
@description('Restart a deallocated (evicted) VM on this cadence.')
param watchdogMinutes int = 10

var suffix = uniqueString(resourceGroup().id)
var tags = { project: 'kaveon', environment: 'demo', owner: 'prproddu' }
var storageBlobDataReader = '2a2b9908-6ea1-4ae2-8e65-a410df84e5d1'
var acrPull = '7f951dda-4ed3-4680-a7ca-43fe172d538d'
var vmContributor = '9980e02c-c2be-4d73-94e8-173b1dc7cf3c'

resource registry 'Microsoft.ContainerRegistry/registries@2023-07-01' existing = {
  name: registryName
}

// ── Network ──────────────────────────────────────────────────────────────────
resource nsg 'Microsoft.Network/networkSecurityGroups@2024-05-01' = {
  name: 'kaveon-demo-nsg'
  location: location
  tags: tags
  properties: {
    securityRules: [
      { name: 'ssh-operator', properties: { priority: 100, direction: 'Inbound', access: 'Allow', protocol: 'Tcp', sourceAddressPrefix: operatorIpCidr, sourcePortRange: '*', destinationAddressPrefix: '*', destinationPortRange: '22' } }
      { name: 'https', properties: { priority: 110, direction: 'Inbound', access: 'Allow', protocol: 'Tcp', sourceAddressPrefix: 'Internet', sourcePortRange: '*', destinationAddressPrefix: '*', destinationPortRange: '443' } }
      { name: 'acme-http', properties: { priority: 120, direction: 'Inbound', access: 'Allow', protocol: 'Tcp', sourceAddressPrefix: 'Internet', sourcePortRange: '*', destinationAddressPrefix: '*', destinationPortRange: '80' } }
    ]
  }
}

resource network 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: 'kaveon-demo-vnet'
  location: location
  tags: tags
  properties: {
    addressSpace: { addressPrefixes: ['10.60.0.0/24'] }
    subnets: [
      {
        name: 'vm'
        properties: {
          addressPrefix: '10.60.0.0/26'
          networkSecurityGroup: { id: nsg.id }
          serviceEndpoints: [{ service: 'Microsoft.Storage' }]
        }
      }
    ]
  }
}

resource publicIp 'Microsoft.Network/publicIPAddresses@2024-05-01' = {
  name: 'kaveon-demo-ip'
  location: location
  tags: tags
  sku: { name: 'Standard' }
  properties: {
    publicIPAllocationMethod: 'Static'
    dnsSettings: { domainNameLabel: dnsLabel }
  }
}

resource nic 'Microsoft.Network/networkInterfaces@2024-05-01' = {
  name: 'kaveon-demo-nic'
  location: location
  tags: tags
  properties: {
    ipConfigurations: [
      {
        name: 'primary'
        properties: {
          subnet: { id: network.properties.subnets[0].id }
          privateIPAllocationMethod: 'Dynamic'
          publicIPAddress: { id: publicIp.id }
        }
      }
    ]
  }
}

// ── Lake ─────────────────────────────────────────────────────────────────────
resource lake 'Microsoft.Storage/storageAccounts@2023-05-01' = {
  name: 'kvdemo${suffix}'
  location: location
  tags: tags
  kind: 'StorageV2'
  sku: { name: 'Standard_LRS' }
  properties: {
    isHnsEnabled: true
    minimumTlsVersion: 'TLS1_2'
    supportsHttpsTrafficOnly: true
    allowBlobPublicAccess: false
    allowSharedKeyAccess: false
    networkAcls: {
      defaultAction: 'Deny'
      bypass: 'AzureServices'
      ipRules: [{ value: split(operatorIpCidr, '/')[0], action: 'Allow' }]
      virtualNetworkRules: [{ id: network.properties.subnets[0].id, action: 'Allow' }]
    }
  }
}
resource lakeBlobs 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' = {
  parent: lake
  name: 'default'
}
resource lakeContainers 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = [for name in ['opensource', 'product-transactions']: {
  parent: lakeBlobs
  name: name
  properties: { publicAccess: 'None' }
}]

// ── Virtual machine ───────────────────────────────────────────────────────────
var cloudInit = base64(loadTextContent('../cloud-init/demo-vm.yaml'))

resource vm 'Microsoft.Compute/virtualMachines@2024-07-01' = {
  name: 'kaveon-demo'
  location: location
  tags: tags
  identity: { type: 'SystemAssigned' }
  properties: {
    hardwareProfile: { vmSize: vmSize }
    priority: spot ? 'Spot' : 'Regular'
    evictionPolicy: spot ? 'Deallocate' : null
    billingProfile: spot ? { maxPrice: -1 } : null
    storageProfile: {
      imageReference: { publisher: 'Canonical', offer: 'ubuntu-24_04-lts', sku: 'server', version: 'latest' }
      osDisk: {
        name: 'kaveon-demo-os'
        createOption: 'FromImage'
        diskSizeGB: 128
        managedDisk: { storageAccountType: 'Premium_LRS' }
        deleteOption: 'Detach'
      }
    }
    osProfile: {
      computerName: 'kaveon-demo'
      adminUsername: adminUsername
      customData: cloudInit
      linuxConfiguration: {
        disablePasswordAuthentication: true
        ssh: { publicKeys: [{ path: '/home/${adminUsername}/.ssh/authorized_keys', keyData: sshPublicKey }] }
        patchSettings: { patchMode: 'AutomaticByPlatform', assessmentMode: 'AutomaticByPlatform' }
      }
    }
    networkProfile: { networkInterfaces: [{ id: nic.id }] }
    diagnosticsProfile: { bootDiagnostics: { enabled: true } }
  }
}

resource lakeReader 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(lake.id, vm.id, storageBlobDataReader)
  scope: lake
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', storageBlobDataReader)
    principalId: vm.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

resource imagePull 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(registry.id, vm.id, acrPull)
  scope: registry
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', acrPull)
    principalId: vm.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

// ── Watchdog: start the VM again after a Spot eviction ────────────────────────
resource watchdog 'Microsoft.Logic/workflows@2019-05-01' = {
  name: 'kaveon-demo-watchdog'
  location: location
  tags: tags
  identity: { type: 'SystemAssigned' }
  properties: {
    state: 'Enabled'
    definition: {
      '$schema': 'https://schema.management.azure.com/providers/Microsoft.Logic/schemas/2016-06-01/workflowdefinition.json#'
      contentVersion: '1.0.0.0'
      parameters: {}
      triggers: {
        every: { type: 'Recurrence', recurrence: { frequency: 'Minute', interval: watchdogMinutes } }
      }
      actions: {
        instanceView: {
          type: 'Http'
          inputs: {
            method: 'GET'
            uri: '${environment().resourceManager}${vm.id}/instanceView?api-version=2024-07-01'
            authentication: { type: 'ManagedServiceIdentity', audience: environment().resourceManager }
          }
        }
        startIfDeallocated: {
          type: 'If'
          runAfter: { instanceView: ['Succeeded'] }
          expression: {
            and: [
              { contains: ['@string(body(\'instanceView\')?[\'statuses\'])', 'PowerState/deallocated'] }
            ]
          }
          actions: {
            start: {
              type: 'Http'
              inputs: {
                method: 'POST'
                uri: '${environment().resourceManager}${vm.id}/start?api-version=2024-07-01'
                authentication: { type: 'ManagedServiceIdentity', audience: environment().resourceManager }
              }
            }
          }
        }
      }
      outputs: {}
    }
  }
}

resource watchdogRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(vm.id, watchdog.id, vmContributor)
  scope: vm
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', vmContributor)
    principalId: watchdog.identity.principalId
    principalType: 'ServicePrincipal'
  }
}

output apiHost string = publicIp.properties.dnsSettings.fqdn
output publicIp string = publicIp.properties.ipAddress
output lakeAccount string = lake.name
output vmPrincipalId string = vm.identity.principalId
