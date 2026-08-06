@description('Name of the Log Analytics workspace.')
param name string

@description('Azure region for the resource.')
param location string

resource workspace 'Microsoft.OperationalInsights/workspaces@2022-10-01' = {
  name: name
  location: location
  properties: {
    sku: {
      name: 'PerGB2018'
    }
    retentionInDays: 30
  }
}

output workspaceId string = workspace.id
output workspaceCustomerId string = workspace.properties.customerId
output primarySharedKey string = workspace.listKeys().primarySharedKey
