{{- define "portal.image" -}}
{{- $image := index . 0 -}}{{- $name := index . 1 -}}
{{- $repo := required (printf "images.%s.repository is required" $name) $image.repository -}}
{{- $digest := required (printf "images.%s.digest is required" $name) $image.digest -}}
{{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) }}{{ fail (printf "images.%s.digest must be an immutable sha256 digest" $name) }}{{ end -}}
{{ printf "%s@%s" $repo $digest }}
{{- end -}}

{{- /*
  The API process environment, in one place.

  The scheduled job that recompiles DLM context runs the API image and talks
  to the same Engine, the same object storage and the same catalog the API
  does — so every one of these settings has to agree with the API's, and two
  copies of a fifty-line env block do not stay in agreement. A job that
  disagreed about, say, KAVEON_POSTGRESQL_RETIREMENT_MODE would read a
  different system of record than the API whose context it is refreshing, and
  nothing would say so.
*/ -}}
{{- define "portal.apiEnv" -}}
{{- $retirement := .Values.api.cutover.retirementMode -}}
{{- $rehearsal := .Values.api.cutover.restartRehearsalMode -}}
- {name: NODE_ENV, value: production}
- {name: KAVEON_DEMO_MODE, value: {{ .Values.api.demoMode | quote }}}
{{- if not (or $retirement $rehearsal) }}
- {name: METADATA_DB_TYPE, value: postgresql}
- {name: METADATA_HOST, value: kaveon-postgres}
- {name: METADATA_PORT, value: "5432"}
- {name: METADATA_DATABASE, value: kaveonmeta}
- {name: METADATA_USER, value: kaveon}
- {name: METADATA_SSLMODE, value: disable}
- {name: METADATA_PASSWORD, valueFrom: {secretKeyRef: {name: {{ required "secrets.portalAuth is required" .Values.secrets.portalAuth }}, key: METADATA_PASSWORD}}}
{{- end }}
- {name: KAVEON_KEY_VAULT_URL, value: {{ required "api.keyVaultUrl is required" .Values.api.keyVaultUrl | quote }}}
- {name: KAVEON_PRODUCT_REPLAY_ENABLED, value: {{ .Values.api.productReplay.enabled | quote }}}
- {name: KAVEON_PRODUCT_REPLAY_BATCH_SIZE, value: {{ .Values.api.productReplay.batchSize | quote }}}
- {name: KAVEON_PRODUCT_REPLAY_INTERVAL_SECONDS, value: {{ .Values.api.productReplay.intervalSeconds | quote }}}
- {name: KAVEON_POSTGRESQL_RETIREMENT_MODE, value: {{ $retirement | quote }}}
- {name: KAVEON_POSTGRESQL_RESTART_REHEARSAL_MODE, value: {{ $rehearsal | quote }}}
- {name: KAVEONDB_AUTHORITY_FAMILIES, value: {{ .Values.api.cutover.authorityFamilies | quote }}}
- {name: KAVEONDB_READ_AUTHORITY_FAMILIES, value: {{ .Values.api.cutover.readAuthorityFamilies | quote }}}
- {name: KAVEON_POSTGRESQL_RETIREMENT_EVIDENCE, value: {{ .Values.api.cutover.evidencePath | quote }}}
- {name: KAVEON_POSTGRESQL_RETIREMENT_AUDIT, value: {{ .Values.api.cutover.auditPath | quote }}}
- {name: KAVEON_POSTGRESQL_RECONCILIATION_REPORTS, value: {{ .Values.api.cutover.reportDirectory | quote }}}
- {name: KAVEON_POSTGRESQL_OPERATIONAL_RECEIPTS, value: {{ .Values.api.cutover.receiptDirectory | quote }}}
- {name: KAVEON_POSTGRESQL_RETIREMENT_MAX_AGE_HOURS, value: {{ .Values.api.cutover.maxAgeHours | quote }}}
- {name: KAVEON_POSTGRESQL_WRITE_FENCE_ENABLED, value: {{ .Values.api.cutover.postgresqlWriteFence | quote }}}
- {name: KAVEON_DLM_LIVE_ARTIFACT_PUBLISH_ENABLED, value: {{ .Values.api.cutover.liveDlmArtifactPublish | quote }}}
- {name: KAVEON_ADLS_ACCOUNT, value: {{ .Values.api.cutover.artifactAccount | quote }}}
- {name: KAVEON_ADLS_CONTAINER, value: {{ .Values.api.cutover.artifactContainer | quote }}}
- {name: KAVEON_SOURCE_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.sources | quote }}}
- {name: KAVEON_DATASET_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.datasets | quote }}}
- {name: KAVEON_CHART_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.charts | quote }}}
- {name: KAVEON_DASHBOARD_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.dashboards | quote }}}
- {name: KAVEON_USER_THEME_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.userThemes | quote }}}
- {name: KAVEON_FAVORITE_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.favorites | quote }}}
- {name: KAVEON_USER_RECENT_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.userRecents | quote }}}
- {name: KAVEON_QUERY_HISTORY_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.queryHistory | quote }}}
- {name: KAVEON_SAVED_QUERY_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.savedQueries | quote }}}
- {name: KAVEON_ACTIVITY_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.activity | quote }}}
- {name: KAVEON_CHAT_HISTORY_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.chatHistory | quote }}}
- {name: KAVEON_DLM_DEFINITION_SHADOW_READ_ENABLED, value: {{ .Values.api.cutover.shadowReads.dlmDefinitions | quote }}}
- {name: KAVEON_CHART_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.charts | quote }}}
- {name: KAVEON_DASHBOARD_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.dashboards | quote }}}
- {name: KAVEON_SAVED_QUERY_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.savedQueries | quote }}}
- {name: KAVEON_QUERY_HISTORY_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.queryHistory | quote }}}
- {name: KAVEON_ACTIVITY_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.activity | quote }}}
- {name: KAVEON_CHAT_HISTORY_OUTBOX_ENABLED, value: {{ .Values.api.cutover.outbox.chatHistory | quote }}}
- {name: KAVEON_ENGINE_URL, value: "https://{{ .Values.engine.service }}:{{ .Values.engine.port }}"}
- {name: KAVEON_ENGINE_CA_CERT, value: /var/run/kaveon/engine-ca/{{ .Values.engine.caKey }}}
- {name: KAVEON_PROXY_SECRET, valueFrom: {secretKeyRef: {name: {{ .Values.secrets.portalAuth }}, key: KAVEON_PROXY_SECRET}}}
- {name: KAVEON_ENGINE_BRIDGE_TOKEN, valueFrom: {secretKeyRef: {name: {{ .Values.secrets.portalAuth }}, key: KAVEON_ENGINE_BRIDGE_TOKEN}}}
- {name: KAVEON_ENGINE_CATALOG_TOKEN, valueFrom: {secretKeyRef: {name: {{ .Values.secrets.portalAuth }}, key: KAVEON_ENGINE_CATALOG_TOKEN}}}
- {name: KAVEON_CREDENTIAL_ACTIVE_KEY, valueFrom: {secretKeyRef: {name: {{ .Values.secrets.portalAuth }}, key: KAVEON_CREDENTIAL_ACTIVE_KEY}}}
- {name: KAVEON_CREDENTIAL_KEYS, valueFrom: {secretKeyRef: {name: {{ .Values.secrets.portalAuth }}, key: KAVEON_CREDENTIAL_KEYS}}}
{{- end -}}
