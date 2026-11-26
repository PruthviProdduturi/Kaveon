targetScope = 'resourceGroup'

param publicIpName string
param clusterIdentityName string

resource edgeIp 'Microsoft.Network/publicIPAddresses@2024-05-01' existing = {
  name: publicIpName
}
resource clusterIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' existing = {
  name: clusterIdentityName
}

// The AKS cloud controller binds this reserved IP to the Studio LoadBalancer.
resource bindPublicIp 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(edgeIp.id, clusterIdentity.id, 'studio-edge')
  scope: edgeIp
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '4d97b98b-1d4f-4787-a291-c67834d212e7')
    principalId: clusterIdentity.properties.principalId
    principalType: 'ServicePrincipal'
  }
}
