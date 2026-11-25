targetScope = 'resourceGroup'

@minLength(3)
@maxLength(16)
@description('Installation name. Use 3–16 lowercase letters, numbers or hyphens. Start with a letter and end with a letter or number.')
param installationName string = 'kaveon'

@description('Azure region for the Kaveon deployment.')
param location string = resourceGroup().location
@allowed(['dev', 'stage', 'prod'])
param environment string = 'dev'
@description('Globally unique ADLS Gen2 storage account name.')
param storageAccountName string = 'stkv${environment}${installationName == 'kaveon' ? uniqueString(resourceGroup().id) : uniqueString(resourceGroup().id, installationName)}'
@description('Globally unique Azure Container Registry name.')
param registryName string = 'acrkaveon${environment}${installationName == 'kaveon' ? uniqueString(resourceGroup().id) : uniqueString(resourceGroup().id, installationName)}'
@description('Object ID of the initial Azure RBAC cluster operator.')
param operatorObjectId string = deployer().objectId
@description('Create the dedicated Kaveon Entra application and assign its first Admin. Requires directory permission. Disable for an infrastructure-only deployment.')
param configureDirectory bool = true
@description('Kaveon Admin user or group object ID. Defaults to the cluster operator.')
param kaveonAdminObjectId string = operatorObjectId
@description('AKS worker VM size.')
param nodeSize string = 'Standard_B2s'
@description('Minimum autoscaled worker count.')
param minWorkers int = 1
@description('Maximum autoscaled worker count.')
param maxWorkers int = 2
@description('Create a dedicated NAT Gateway. Disabled by default to avoid unnecessary monthly cost.')
param enableNatGateway bool = false

var resourcePrefix = '${installationName}-${environment}'
var installKey = uniqueString(resourceGroup().id, resourcePrefix)
var studioDnsLabel = '${resourcePrefix}-${installKey}'
// Claim the DNS name before registering it as a trusted callback.
resource studioIp 'Microsoft.Network/publicIPAddresses@2024-05-01' = if (configureDirectory) {
  name: '${resourcePrefix}-studio-ip'
  location: location
  tags: tags
  sku: { name: 'Standard' }
  properties: {
    publicIPAllocationMethod: 'Static'
    publicIPAddressVersion: 'IPv4'
    dnsSettings: {
      domainNameLabel: studioDnsLabel
      domainNameLabelScope: 'ResourceGroupReuse'
    }
  }
}

var studioOrigin = configureDirectory ? 'https://${studioIp!.properties.dnsSettings!.fqdn!}' : ''

module directory 'directory.bicep' = if (configureDirectory) {
  name: 'directory-${installKey}'
  params: {
    installationKey: 'kaveon-${installKey}'
    installationName: resourcePrefix
    studioOrigin: studioOrigin
    adminPrincipalId: kaveonAdminObjectId
  }
}

var tags = { product: 'kaveon', environment: environment, managedBy: 'kaveon-marketplace', architecture: 'kaveondb' }

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
  name: 'aks-${installKey}'
  params: {
    storageAccountName: storage.name
    registryName: registry.name
    location: location
    operatorObjectId: operatorObjectId
    resourcePrefix: resourcePrefix
    nodeResourceGroupName: 'rg-${resourcePrefix}-nodes'
    systemNodeSize: 'Standard_D4s_v5'
    systemNodeCount: 2
    tags: tags
    nodeSize: nodeSize
    minWorkers: minWorkers
    maxWorkers: maxWorkers
    enableNatGateway: enableNatGateway
  }
  dependsOn: [openSourceContainer, productContainer, backupContainer, directory]
}

output clusterName string = aks.outputs.clusterName
output registryName string = registry.name
output storageAccountName string = storage.name
output adlsOpenSourceContainer string = openSourceContainer.name
output adlsProductContainer string = productContainer.name
output adlsBackupContainer string = backupContainer.name
output readerClientId string = aks.outputs.readerClientId
@description('Expected origin to configure on the ingress. It becomes reachable after the workloads and TLS are deployed.')
output plannedStudioOrigin string = studioOrigin
output studioPublicIpName string = configureDirectory ? studioIp!.name : ''
output entraHelmValues object = configureDirectory ? {
  entraAccess: { tenantId: tenant().tenantId, clientId: directory!.outputs.clientId, catalogs: [] }
  portal: {
    publicUrl: studioOrigin
    entraPublicClient: true
    entraAccess: { tenantId: tenant().tenantId, clientId: directory!.outputs.clientId }
  }
} : {}
output entraClientId string = directory.?outputs.clientId ?? ''
output deploymentMode string = 'KaveonDB'
