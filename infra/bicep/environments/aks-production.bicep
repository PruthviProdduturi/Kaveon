targetScope = 'resourceGroup'

@description('Existing ADLS Gen2 account. No data resources are recreated by this template.')
param storageAccountName string
@description('Existing ACR containing the immutable Kaveon images.')
param registryName string
param location string = resourceGroup().location
param kubernetesVersion string = '1.35.7'
param operatorObjectId string
param nodeSize string = 'Standard_D4s_v3'
param minWorkers int = 1
param maxWorkers int = 4

var tags = {
  project: 'kaveon'
  environment: 'production'
  owner: 'prproddu'
}

resource storage 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}
resource registry 'Microsoft.ContainerRegistry/registries@2023-07-01' existing = {
  name: registryName
}

resource network 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: 'kaveon-vnet'
  location: location
  tags: tags
  properties: { addressSpace: { addressPrefixes: ['10.225.0.0/16'] } }
}
resource egressIp 'Microsoft.Network/publicIPAddresses@2024-05-01' = {
  name: 'kaveon-egress-ip'
  location: location
  tags: tags
  sku: { name: 'Standard', tier: 'Regional' }
  properties: { publicIPAllocationMethod: 'Static', publicIPAddressVersion: 'IPv4' }
}
resource egress 'Microsoft.Network/natGateways@2024-05-01' = {
  name: 'kaveon-egress'
  location: location
  tags: tags
  sku: { name: 'Standard' }
  properties: {
    idleTimeoutInMinutes: 10
    publicIpAddresses: [{ id: egressIp.id }]
  }
}
resource subnet 'Microsoft.Network/virtualNetworks/subnets@2024-05-01' = {
  parent: network
  name: 'aks'
  properties: {
    addressPrefix: '10.225.0.0/20'
    defaultOutboundAccess: false
    natGateway: { id: egress.id }
    serviceEndpoints: [
      { service: 'Microsoft.Storage' }
      { service: 'Microsoft.KeyVault' }
    ]
  }
}

resource identity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'kaveon-cluster'
  location: location
  tags: tags
}
resource networkRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: network
  name: guid(network.id, identity.id, 'network')
  properties: {
    principalId: identity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '4d97b98b-1d4f-4787-a291-c67834d212e7')
  }
}

resource cluster 'Microsoft.ContainerService/managedClusters@2025-01-01' = {
  name: 'kaveon-aks'
  location: location
  tags: tags
  sku: { name: 'Base', tier: 'Free' }
  identity: { type: 'UserAssigned', userAssignedIdentities: { '${identity.id}': {} } }
  properties: {
    dnsPrefix: 'kaveon-${uniqueString(resourceGroup().id)}'
    kubernetesVersion: kubernetesVersion
    enableRBAC: true
    disableLocalAccounts: true
    aadProfile: { managed: true, enableAzureRBAC: true, tenantID: tenant().tenantId }
    apiServerAccessProfile: { authorizedIPRanges: [] }
    oidcIssuerProfile: { enabled: true }
    securityProfile: { workloadIdentity: { enabled: true } }
    agentPoolProfiles: [
      {
        name: 'system'
        count: 1
        vmSize: nodeSize
        osType: 'Linux'
        osSKU: 'Ubuntu'
        mode: 'System'
        type: 'VirtualMachineScaleSets'
        osDiskSizeGB: 64
        maxPods: 50
        vnetSubnetID: subnet.id
      }
      {
        name: 'workers'
        count: minWorkers
        enableAutoScaling: true
        minCount: minWorkers
        maxCount: maxWorkers
        vmSize: nodeSize
        osType: 'Linux'
        osSKU: 'Ubuntu'
        mode: 'User'
        type: 'VirtualMachineScaleSets'
        osDiskSizeGB: 64
        maxPods: 50
        vnetSubnetID: subnet.id
        nodeLabels: { workload: 'kaveon-worker' }
      }
    ]
    networkProfile: {
      networkPlugin: 'azure'
      networkPluginMode: 'overlay'
      networkPolicy: 'azure'
      podCidr: '10.244.0.0/16'
      serviceCidr: '10.0.0.0/16'
      dnsServiceIP: '10.0.0.10'
      loadBalancerSku: 'standard'
      outboundType: 'userAssignedNATGateway'
    }
  }
  dependsOn: [networkRole]
}
resource pullRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: registry
  name: guid(registry.id, cluster.id, 'pull')
  properties: {
    principalId: cluster.properties.identityProfile.kubeletidentity.objectId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '7f951dda-4ed3-4680-a7ca-43fe172d538d')
  }
}
resource operatorRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: cluster
  name: guid(cluster.id, operatorObjectId, 'admin')
  properties: {
    principalId: operatorObjectId
    principalType: 'User'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b1ff04bb-8a4e-4dc4-8eb5-8693973ce19b')
  }
}
resource readerIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'kaveon-reader'
  location: location
  tags: tags
}
resource federation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = {
  parent: readerIdentity
  name: 'kaveon-engine'
  properties: {
    issuer: cluster.properties.oidcIssuerProfile.issuerURL
    subject: 'system:serviceaccount:kaveon:kaveon-engine'
    audiences: ['api://AzureADTokenExchange']
  }
}
resource readerRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: storage
  name: guid(storage.id, readerIdentity.id, 'read')
  properties: {
    principalId: readerIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '2a2b9908-6ea1-4ae2-8e65-a410df84e7d1')
  }
}

output clusterName string = cluster.name
output registryName string = registry.name
output storageAccountName string = storage.name
output readerClientId string = readerIdentity.properties.clientId
output workerMin string = string(minWorkers)
output workerMax string = string(maxWorkers)
