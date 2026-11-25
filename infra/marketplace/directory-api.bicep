extension 'br:mcr.microsoft.com/bicep/extensions/microsoftgraph/v1.0:1.0.0' as graph

param installationKey string
param displayName string
param clientId string
param scopeId string

// The issuer assigns appId on creation. Use it in the resource URI in a second deployment.
resource api 'graph:Microsoft.Graph/applications@v1.0' = {
  uniqueName: installationKey
  displayName: displayName
  identifierUris: ['api://${clientId}']
  requiredResourceAccess: [
    {
      resourceAppId: clientId
      resourceAccess: [{ id: scopeId, type: 'Scope' }]
    }
  ]
}
