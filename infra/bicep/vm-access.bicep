// Access a Kaveon host needs: read and write its system storage, and pull its
// own images.
//
// Expressed as a deployment rather than `az role assignment create` because
// that command is broken in several Azure CLI builds (it answers
// MissingSubscription against a fully qualified scope while the Authorization
// provider is registered and reachable). A deployment goes through
// Microsoft.Resources and works regardless, and — being checked in — a second
// host gets the same access by running the same thing.
//
//   az deployment group create -g <rg> --template-file infra/bicep/vm-access.bicep \
//     --parameters principalId=<vm identity> storageAccountName=<lake> registryName=<acr>

@description('Object (principal) ID of the host\'s managed identity.')
param principalId string

@description('Storage account holding the system storage.')
param storageAccountName string

@description('Container registry the host pulls images from. Empty to skip.')
param registryName string = ''

@description('''What the principal is. A host's managed identity is a
ServicePrincipal; a person granting themselves the same access for an
operational task is a User, and Azure refuses the assignment when this does
not match the principal it finds.''')
@allowed(['ServicePrincipal', 'User', 'Group'])
param principalType string = 'ServicePrincipal'

// Built-in role definition IDs are stable across clouds and tenants.
var storageBlobDataContributor = 'ba92f5b4-2d11-453d-a403-e96b0029c9fe'
var acrPull = '7f951dda-4ed3-4680-a7ca-43fe172d538d'

resource lake 'Microsoft.Storage/storageAccounts@2023-01-01' existing = {
  name: storageAccountName
}

resource registry 'Microsoft.ContainerRegistry/registries@2023-07-01' existing = if (!empty(registryName)) {
  name: registryName
}

// The name must be a GUID that is deterministic in the assignment's identity,
// so re-running the deployment is idempotent instead of failing on a duplicate.
resource lakeAccess 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  scope: lake
  name: guid(lake.id, principalId, storageBlobDataContributor)
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', storageBlobDataContributor)
    principalId: principalId
    principalType: principalType
  }
}

resource registryAccess 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(registryName)) {
  scope: registry
  name: guid(registry.id, principalId, acrPull)
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', acrPull)
    principalId: principalId
    principalType: principalType
  }
}

output lakeRoleAssignmentId string = lakeAccess.id
output registryRoleAssignmentId string = empty(registryName) ? '' : registryAccess.id
