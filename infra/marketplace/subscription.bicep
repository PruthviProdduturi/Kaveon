targetScope = 'subscription'

param location string
@allowed(['dev', 'stage', 'prod'])
param environment string = 'dev'
param operatorObjectId string

resource appGroup 'Microsoft.Resources/resourceGroups@2024-03-01' = {
  name: 'rg-kaveon-${environment}'
  location: location
  tags: { product: 'kaveon', environment: environment, managedBy: 'kaveon-marketplace' }
}

module foundation 'main.bicep' = {
  name: 'kaveon-foundation'
  scope: appGroup
  params: {
    environment: environment
    location: location
    operatorObjectId: operatorObjectId
  }
}

output applicationResourceGroup string = appGroup.name
output clusterName string = foundation.outputs.clusterName
output storageAccountName string = foundation.outputs.storageAccountName
output registryName string = foundation.outputs.registryName
