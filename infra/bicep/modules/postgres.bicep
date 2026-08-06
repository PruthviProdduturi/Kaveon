@description('Name of the PostgreSQL Flexible Server.')
param name string

@description('Azure region for the resource.')
param location string

@description('Administrator login name.')
param adminUser string

@description('Administrator login password.')
@secure()
param adminPassword string

@description('Storage size in GB.')
param storageSizeGB int = 32

@description('SKU name for the server (e.g. Standard_B1ms).')
param skuName string = 'Standard_B1ms'

resource server 'Microsoft.DBforPostgreSQL/flexibleServers@2023-06-01-preview' = {
  name: name
  location: location
  sku: {
    name: skuName
    tier: 'Burstable'
  }
  properties: {
    administratorLogin: adminUser
    administratorLoginPassword: adminPassword
    storage: {
      storageSizeGB: storageSizeGB
    }
    version: '15'
    network: {
      publicNetworkAccess: 'Enabled'
    }
    backup: {
      backupRetentionDays: 7
      geoRedundantBackup: 'Disabled'
    }
    highAvailability: {
      mode: 'Disabled'
    }
  }
}

resource database 'Microsoft.DBforPostgreSQL/flexibleServers/databases@2023-06-01-preview' = {
  parent: server
  name: 'kaveon'
  properties: {
    charset: 'UTF8'
    collation: 'en_US.utf8'
  }
}

output serverFqdn string = server.properties.fullyQualifiedDomainName
output serverName string = server.name
