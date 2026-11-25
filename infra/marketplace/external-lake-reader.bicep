targetScope = 'resourceGroup'

@description('Deploy in the resource group containing the existing analytical lake.')
param storageAccountName string
@description('Exact oidcIssuerProfile.issuerUrl returned by the authorized work AKS cluster.')
param clusterIssuer string
@description('Identity dedicated to this cluster and namespace.')
param identityName string
@minLength(1)
param analyticsContainers array

resource lake 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}
resource service 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' existing = {
  parent: lake
  name: 'default'
}
resource containers 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' existing = [for container in analyticsContainers: {
  parent: service
  name: container
}]
resource reader 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: identityName
  location: resourceGroup().location
}
resource credential 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = {
  parent: reader
  name: 'engine'
  properties: {
    issuer: clusterIssuer
    subject: 'system:serviceaccount:kaveon:kaveon-engine'
    audiences: ['api://AzureADTokenExchange']
  }
}
var blobDataReader = subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '2a2b9908-6ea1-4ae2-8e65-a410df84e7d1')
resource grants 'Microsoft.Authorization/roleAssignments@2022-04-01' = [for (container, i) in analyticsContainers: {
  scope: containers[i]
  name: guid(lake.id, container, reader.id, blobDataReader)
  properties: {
    roleDefinitionId: blobDataReader
    principalId: reader.properties.principalId
    principalType: 'ServicePrincipal'
  }
}]

output externalLake object = {
  account: lake.name
  clientId: reader.properties.clientId
  tenantId: tenant().tenantId
}
