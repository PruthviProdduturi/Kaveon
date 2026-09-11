targetScope = 'resourceGroup'

@description('Existing AKS cluster with OIDC issuer and workload identity enabled.')
param clusterName string

@minLength(1)
@description('Stable AKS outbound public IPv4 CIDRs allowed to reach Key Vault.')
param apiEgressIpCidrs array

param location string = resourceGroup().location
param identityName string = 'kaveon-test-api'
param serviceAccountNamespace string = 'kaveon'
param serviceAccountName string = 'kaveon-api'
param vaultName string = 'kvsec${uniqueString(resourceGroup().id)}'

var tags = { project: 'kaveon', environment: 'test', component: 'api-secrets' }

resource cluster 'Microsoft.ContainerService/managedClusters@2025-01-01' existing = {
  name: clusterName
}

resource apiIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: identityName
  location: location
  tags: tags
}

resource apiFederation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = {
  parent: apiIdentity
  name: 'kaveon-api'
  properties: {
    issuer: cluster.properties.oidcIssuerProfile.issuerURL
    subject: 'system:serviceaccount:${serviceAccountNamespace}:${serviceAccountName}'
    audiences: ['api://AzureADTokenExchange']
  }
}

resource productSecrets 'Microsoft.KeyVault/vaults@2023-07-01' = {
  name: vaultName
  location: location
  tags: tags
  properties: {
    tenantId: tenant().tenantId
    sku: { family: 'A', name: 'standard' }
    enableRbacAuthorization: true
    enableSoftDelete: true
    softDeleteRetentionInDays: 7
    publicNetworkAccess: 'Enabled'
    networkAcls: {
      bypass: 'AzureServices'
      defaultAction: 'Deny'
      ipRules: [for cidr in apiEgressIpCidrs: { value: cidr }]
      virtualNetworkRules: []
    }
  }
}

resource apiSecretsRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: productSecrets
  name: guid(productSecrets.id, apiIdentity.id, 'secrets-officer')
  properties: {
    principalId: apiIdentity.properties.principalId
    principalType: 'ServicePrincipal'
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', 'b86a8fe4-44ce-4948-aee5-eccb2c155cd7')
  }
}

output apiClientId string = apiIdentity.properties.clientId
output productSecretsVaultName string = productSecrets.name
output productSecretsVaultUri string = productSecrets.properties.vaultUri
