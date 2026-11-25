targetScope = 'resourceGroup'

@description('Azure region for the Kaveon deployment.')
param location string = resourceGroup().location
@description('Globally unique ADLS Gen2 storage account name.')
param storageAccountName string
@description('Globally unique Azure Container Registry name.')
param registryName string
@description('Object ID of the initial Azure RBAC cluster operator.')
param operatorObjectId string
@description('AKS worker VM size.')
param nodeSize string = 'Standard_B2s'
@description('Minimum autoscaled worker count.')
param minWorkers int = 1
@description('Maximum autoscaled worker count.')
param maxWorkers int = 2
@description('Create a dedicated NAT Gateway. Disabled by default to avoid unnecessary monthly cost.')
param enableNatGateway bool = false

var tags = { product: 'kaveon', managedBy: 'kaveon-marketplace', architecture: 'kaveondb' }

resource storage 'Microsoft.Storage/storageAccounts@2023-05-01' = {
  name: storageAccountName
  location: location
  tags: tags
  kind: 'StorageV2'
  sku: { name: 'Standard_LRS' }
  properties: {
    isHnsEnabled: true
    minimumTlsVersion: 'TLS1_2'
    allowBlobPublicAccess: false
    allowSharedKeyAccess: false
    publicNetworkAccess: 'Enabled'
  }
}
resource blobService 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' = {
  parent: storage
  name: 'default'
}
resource openSourceContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = {
  parent: blobService
  name: 'opensource'
  properties: { publicAccess: 'None' }
}
resource productContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = {
  parent: blobService
  name: 'product'
  properties: { publicAccess: 'None' }
}
resource backupContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = {
  parent: blobService
  name: 'backups'
  properties: { publicAccess: 'None' }
}
resource registry 'Microsoft.ContainerRegistry/registries@2023-07-01' = {
  name: registryName
  location: location
  tags: tags
  sku: { name: 'Basic' }
  properties: { adminUserEnabled: false, publicNetworkAccess: 'Enabled' }
}

module aks '../bicep/environments/aks-production.bicep' = {
  name: 'kaveon-aks'
  params: {
    storageAccountName: storage.name
    registryName: registry.name
    location: location
    operatorObjectId: operatorObjectId
    nodeSize: nodeSize
    minWorkers: minWorkers
    maxWorkers: maxWorkers
    enableNatGateway: enableNatGateway
  }
  dependsOn: [openSourceContainer, productContainer, backupContainer]
}

output clusterName string = aks.outputs.clusterName
output registryName string = registry.name
output storageAccountName string = storage.name
output adlsOpenSourceContainer string = openSourceContainer.name
output adlsProductContainer string = productContainer.name
output adlsBackupContainer string = backupContainer.name
output readerClientId string = aks.outputs.readerClientId
output deploymentMode string = 'PostgreSQL-free KaveonDB authority'
