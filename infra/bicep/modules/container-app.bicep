@description('Name of the Container App.')
param name string

@description('Azure region for the resource.')
param location string

@description('Resource ID of the Container Apps environment.')
param environmentId string

@description('Container registry login server (e.g. kaveonacr.azurecr.io).')
param registryServer string

@description('Container registry admin username.')
param registryUsername string

@description('Container registry admin password.')
@secure()
param registryPassword string

@description('Full image name including tag (e.g. kaveonacr.azurecr.io/kaveon-api:latest).')
param imageName string

@description('Environment variables for the container. Use {name, value} for plain values and {name, secretRef} for secrets.')
param envVars array = []

resource app 'Microsoft.App/containerApps@2023-05-01' = {
  name: name
  location: location
  properties: {
    environmentId: environmentId
    configuration: {
      ingress: {
        external: true
        targetPort: 8080
        transport: 'http'
        allowInsecure: false
      }
      registries: [
        {
          server: registryServer
          username: registryUsername
          passwordSecretRef: 'registry-password'
        }
      ]
      secrets: [
        {
          name: 'registry-password'
          value: registryPassword
        }
      ]
    }
    template: {
      containers: [
        {
          name: name
          image: imageName
          resources: {
            cpu: json('0.5')
            memory: '1Gi'
          }
          env: envVars
        }
      ]
      scale: {
        minReplicas: 1
        maxReplicas: 3
      }
    }
  }
}

output appFqdn string = app.properties.configuration.ingress.fqdn
output appUrl string = 'https://${app.properties.configuration.ingress.fqdn}'
