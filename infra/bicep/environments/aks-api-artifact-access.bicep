targetScope = 'resourceGroup'

param storageAccountName string
param principalId string

resource storage 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}
resource blobs 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' existing = {
  parent: storage
  name: 'default'
}
var containerNames = ['product', 'backups']
resource containers 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' existing = [for containerName in containerNames: {
  parent: blobs
  name: containerName
}]
resource artifactWriter 'Microsoft.Authorization/roleAssignments@2022-04-01' = [for (containerName, i) in containerNames: {
  name: guid(containers[i].id, principalId, 'kaveon-api-artifacts')
  scope: containers[i]
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'ba92f5b4-2d11-453d-a403-e96b0029c9fe')
    principalId: principalId
    principalType: 'ServicePrincipal'
  }
}]
