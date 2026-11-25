extension 'br:mcr.microsoft.com/bicep/extensions/microsoftgraph/v1.0:1.0.0' as graph

@description('Stable key for this installation in the customer tenant.')
param installationKey string
param installationName string
param studioOrigin string
param serviceManagementReference string
@description('Entra user or security group to receive the initial Kaveon Admin assignment.')
param adminPrincipalId string

var roles = loadJsonContent('../../api/services/entra_roles.json')
var scopeId = guid(tenant().tenantId, installationKey, 'access_as_user')

resource application 'graph:Microsoft.Graph/applications@v1.0' = {
  uniqueName: installationKey
  displayName: 'Kaveon · ${installationName}'
  serviceManagementReference: empty(serviceManagementReference) ? null : serviceManagementReference
  signInAudience: 'AzureADMyOrg'
  isFallbackPublicClient: true
  appRoles: roles
  spa: { redirectUris: ['${studioOrigin}/auth/microsoft'] }
  api: {
    requestedAccessTokenVersion: 2
    oauth2PermissionScopes: [
      {
        id: scopeId
        value: 'access_as_user'
        isEnabled: true
        type: 'User'
        adminConsentDisplayName: 'Access Kaveon as the signed-in user'
        userConsentDisplayName: 'Access Kaveon on your behalf'
        adminConsentDescription: 'Use the assigned Kaveon role.'
        userConsentDescription: 'Use your assigned Kaveon role.'
      }
    ]
  }
}

module apiRegistration 'directory-api.bicep' = {
  name: 'kaveon-directory-api'
  params: {
    installationKey: installationKey
    displayName: application.displayName
    clientId: application.appId
    scopeId: scopeId
    serviceManagementReference: serviceManagementReference
  }
}

resource enterprise 'graph:Microsoft.Graph/servicePrincipals@v1.0' = {
  appId: application.appId
  appRoleAssignmentRequired: true
  dependsOn: [apiRegistration]
}

resource firstAdmin 'graph:Microsoft.Graph/appRoleAssignedTo@v1.0' = {
  principalId: adminPrincipalId
  resourceId: enterprise.id
  appRoleId: first(filter(roles, role => role.value == 'Admin'))!.id
}

output clientId string = application.appId
output tenantId string = tenant().tenantId
