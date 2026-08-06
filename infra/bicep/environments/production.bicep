@description('Azure region for all resources.')
param location string = 'westus2'

@description('PostgreSQL administrator login password.')
@secure()
param dbAdminPassword string

@description('Container registry admin password (populated from ACR module output — pass empty string on first deploy, then re-run).')
@secure()
param registryPassword string = ''

// ── Names ────────────────────────────────────────────────────────────────────

var acrName = 'kaveonacr'
var dbServerName = 'kaveon-db'
var dbAdminUser = 'kaveonadmin'
var kvName = 'kaveon-kv'
var logsName = 'kaveon-logs'
var envName = 'kaveon-env'
var appName = 'kaveon-api'

// ── Log Analytics ─────────────────────────────────────────────────────────────

module logs '../modules/log-analytics.bicep' = {
  name: 'deploy-logs'
  params: {
    name: logsName
    location: location
  }
}

// ── Container Registry ────────────────────────────────────────────────────────

module acr '../modules/container-registry.bicep' = {
  name: 'deploy-acr'
  params: {
    name: acrName
    location: location
  }
}

// ── PostgreSQL ─────────────────────────────────────────────────────────────────

module postgres '../modules/postgres.bicep' = {
  name: 'deploy-postgres'
  params: {
    name: dbServerName
    location: location
    adminUser: dbAdminUser
    adminPassword: dbAdminPassword
    storageSizeGB: 32
    skuName: 'Standard_B1ms'
  }
}

// ── Key Vault ─────────────────────────────────────────────────────────────────

module kv '../modules/key-vault.bicep' = {
  name: 'deploy-kv'
  params: {
    name: kvName
    location: location
    tenantId: subscription().tenantId
    // Replace with the managed identity or service principal object ID post-deploy
    principalId: '00000000-0000-0000-0000-000000000000'
  }
}

// ── Container Apps Environment ────────────────────────────────────────────────

module containerEnv '../modules/container-app-env.bicep' = {
  name: 'deploy-container-env'
  params: {
    name: envName
    location: location
    logAnalyticsWorkspaceId: logs.outputs.workspaceId
    logAnalyticsKey: logs.outputs.primarySharedKey
  }
}

// ── Container App ─────────────────────────────────────────────────────────────

module api '../modules/container-app.bicep' = {
  name: 'deploy-api'
  params: {
    name: appName
    location: location
    environmentId: containerEnv.outputs.environmentId
    registryServer: acr.outputs.loginServer
    registryUsername: acr.outputs.adminUsername
    registryPassword: empty(registryPassword) ? acr.outputs.adminPassword : registryPassword
    imageName: '${acr.outputs.loginServer}/kaveon-api:latest'
    envVars: [
      {
        name: 'DATABASE_URL'
        value: 'postgresql://${dbAdminUser}:$(DB_PASSWORD)@${postgres.outputs.serverFqdn}:5432/kaveon'
      }
      {
        name: 'KEYVAULT_URI'
        value: kv.outputs.vaultUri
      }
      {
        name: 'ENVIRONMENT'
        value: 'production'
      }
    ]
  }
}

// ── Outputs ───────────────────────────────────────────────────────────────────

output apiUrl string = api.outputs.appUrl
output dbFqdn string = postgres.outputs.serverFqdn
output acrLoginServer string = acr.outputs.loginServer
output keyVaultUri string = kv.outputs.vaultUri
