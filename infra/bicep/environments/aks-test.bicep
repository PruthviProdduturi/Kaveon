targetScope = 'resourceGroup'

param location string = resourceGroup().location
param kubernetesVersion string = '1.35.7'
param operatorObjectId string
param operatorIpCidr string
param additionalOperatorIpCidrs array = []
@description('Enable only with stable, verified operator egress CIDRs. Entra authentication and Azure RBAC remain enabled either way.')
param restrictApiToOperatorIps bool = false
param nodeSize string = 'Standard_D4s_v3'

var suffix = uniqueString(resourceGroup().id)
var tags = { project: 'kaveon', environment: 'test', owner: 'prproddu' }

resource registry 'Microsoft.ContainerRegistry/registries@2023-07-01' = {
  name: 'kvtest${suffix}'
  location: location
  tags: tags
  sku: { name: 'Basic' }
  properties: { adminUserEnabled: false }
}

resource storage 'Microsoft.Storage/storageAccounts@2023-05-01' = {
  name: 'kvtest${suffix}'
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
      ipRules: [for cidr in concat([operatorIpCidr], additionalOperatorIpCidrs): { value: split(cidr, '/')[0], action: 'Allow' }]
      virtualNetworkRules: [{ id: subnet.id, action: 'Allow' }]
    }
  }
}
resource blobs 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' = {
  parent: storage
  name: 'default'
}
resource containers 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = [for layer in ['bronze', 'silver', 'gold']: {
  parent: blobs
  name: layer
  properties: { publicAccess: 'None' }
}]

resource network 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: 'kaveon-test-vnet'
  location: location
  tags: tags
  properties: { addressSpace: { addressPrefixes: ['10.224.0.0/16'] } }
}
resource subnet 'Microsoft.Network/virtualNetworks/subnets@2024-05-01' = {
  parent: network
  name: 'aks'
  properties: {
    addressPrefix: '10.224.0.0/20'
    serviceEndpoints: [
      { service: 'Microsoft.Storage' }
      { service: 'Microsoft.KeyVault' }
    ]
  }
}
resource identity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'kaveon-test-cluster'
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
  name: 'kaveon-test-aks'
  location: location
  tags: tags
  sku: { name: 'Base', tier: 'Free' }
  identity: { type: 'UserAssigned', userAssignedIdentities: { '${identity.id}': {} } }
  properties: {
    dnsPrefix: 'kaveon-test-${suffix}'
    kubernetesVersion: kubernetesVersion
    enableRBAC: true
    disableLocalAccounts: true
    aadProfile: { managed: true, enableAzureRBAC: true, tenantID: tenant().tenantId }
    apiServerAccessProfile: {
      authorizedIPRanges: restrictApiToOperatorIps ? concat([operatorIpCidr], additionalOperatorIpCidrs) : []
    }
    oidcIssuerProfile: { enabled: true }
    securityProfile: { workloadIdentity: { enabled: true } }
    agentPoolProfiles: [{
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
    }, {
      name: 'workers'
      count: 3
      vmSize: nodeSize
      osType: 'Linux'
      osSKU: 'Ubuntu'
      mode: 'User'
      type: 'VirtualMachineScaleSets'
      osDiskSizeGB: 64
      maxPods: 50
      vnetSubnetID: subnet.id
      nodeLabels: { workload: 'kaveon-worker' }
    }]
    networkProfile: {
      networkPlugin: 'azure'
      networkPluginMode: 'overlay'
      networkPolicy: 'azure'
      podCidr: '10.244.0.0/16'
      serviceCidr: '10.0.0.0/16'
      dnsServiceIP: '10.0.0.10'
      loadBalancerSku: 'standard'
      outboundType: 'loadBalancer'
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
resource uploaderRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: storage
  name: guid(storage.id, operatorObjectId, 'upload')
  properties: {
    principalId: operatorObjectId
    principalType: 'User'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'ba92f5b4-2d11-453d-a403-e96b0029c9fe')
  }
}
resource readerIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'kaveon-test-reader'
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

resource apiIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: 'kaveon-test-api'
  location: location
  tags: tags
}

resource apiFederation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = {
  parent: apiIdentity
  name: 'kaveon-api'
  properties: {
    issuer: cluster.properties.oidcIssuerProfile.issuerURL
    subject: 'system:serviceaccount:kaveon:kaveon-api'
    audiences: ['api://AzureADTokenExchange']
  }
}

resource productSecrets 'Microsoft.KeyVault/vaults@2023-07-01' = {
  name: 'kvsec${suffix}'
  location: location
  tags: tags
  properties: {
    tenantId: tenant().tenantId
    sku: { family: 'A', name: 'standard' }
    enableRbacAuthorization: true
    enableSoftDelete: true
    softDeleteRetentionInDays: 7
    publicNetworkAccess: 'Enabled'
    networkAcls: {
      bypass: 'AzureServices'
      defaultAction: 'Deny'
      ipRules: []
      virtualNetworkRules: [{ id: subnet.id }]
    }
  }
}

resource apiSecretsRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: productSecrets
  name: guid(productSecrets.id, apiIdentity.id, 'secrets-officer')
  properties: {
    principalId: apiIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b86a8fe4-44ce-4948-aee5-eccb2c155cd7')
  }
}
output clusterName string = cluster.name
output registryName string = registry.name
output storageAccountName string = storage.name
output readerClientId string = readerIdentity.properties.clientId
output apiClientId string = apiIdentity.properties.clientId
output productSecretsVaultName string = productSecrets.name
output productSecretsVaultUri string = productSecrets.properties.vaultUri
