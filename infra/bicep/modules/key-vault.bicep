@description('Name of the Key Vault.')
param name string

@description('Azure region for the resource.')
param location string

@description('Azure AD tenant ID.')
param tenantId string

@description('Object ID of the principal to grant secret access.')
param principalId string

resource kv 'Microsoft.KeyVault/vaults@2023-07-01' = {
  name: name
  location: location
  properties: {
    sku: {
      family: 'A'
      name: 'standard'
    }
    tenantId: tenantId
    enableRbacAuthorization: false
    accessPolicies: [
      {
        tenantId: tenantId
        objectId: principalId
        permissions: {
          secrets: [
            'get'
            'list'
            'set'
            'delete'
          ]
        }
      }
    ]
    enableSoftDelete: true
    softDeleteRetentionInDays: 7
  }
}

output vaultUri string = kv.properties.vaultUri
output vaultName string = kv.name
