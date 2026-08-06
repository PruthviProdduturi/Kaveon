@description('Name of the Container Apps environment.')
param name string

@description('Azure region for the resource.')
param location string

@description('Resource ID of the Log Analytics workspace.')
param logAnalyticsWorkspaceId string

@description('Primary shared key of the Log Analytics workspace.')
@secure()
param logAnalyticsKey string

// Extract the customer ID from the workspace resource
resource logAnalytics 'Microsoft.OperationalInsights/workspaces@2022-10-01' existing = {
  name: last(split(logAnalyticsWorkspaceId, '/'))
}

resource environment 'Microsoft.App/managedEnvironments@2023-05-01' = {
  name: name
  location: location
  properties: {
    appLogsConfiguration: {
      destination: 'log-analytics'
      logAnalyticsConfiguration: {
        customerId: logAnalytics.properties.customerId
        sharedKey: logAnalyticsKey
      }
    }
  }
}

output environmentId string = environment.id
output environmentName string = environment.name
