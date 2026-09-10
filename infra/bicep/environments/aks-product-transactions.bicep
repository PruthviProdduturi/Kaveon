targetScope = 'resourceGroup'

@description('Existing ADLS Gen2 storage account used by Kaveon.')
param storageAccountName string

@description('Existing workload identity used by the Kaveon Engine service account.')
param workloadIdentityName string

@minLength(3)
@maxLength(63)
@description('Dedicated container for product transaction manifests and documents.')
param containerName string = 'product-transactions'

@description('Normalized prefix inside the dedicated container.')
param productPrefix string = 'kaveon/product-catalog'

resource storage 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}

resource blobs 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' existing = {
  parent: storage
  name: 'default'
}

resource identity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' existing = {
  name: workloadIdentityName
}

resource productTransactions 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = {
  parent: blobs
  name: containerName
  properties: { publicAccess: 'None' }
}

resource productTransactionWriterRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: productTransactions
  name: guid(productTransactions.id, identity.id, 'product-transaction-write')
  properties: {
    principalId: identity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'ba92f5b4-2d11-453d-a403-e96b0029c9fe')
  }
}

output storageAccountName string = storage.name
output productTransactionContainer string = productTransactions.name
output productCatalogPrefix string = productPrefix
output readerClientId string = identity.properties.clientId
