{{- define "portal.image" -}}
{{- $image := index . 0 -}}{{- $name := index . 1 -}}
{{- $repo := required (printf "images.%s.repository is required" $name) $image.repository -}}
{{- $digest := required (printf "images.%s.digest is required" $name) $image.digest -}}
{{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) }}{{ fail (printf "images.%s.digest must be an immutable sha256 digest" $name) }}{{ end -}}
{{ printf "%s@%s" $repo $digest }}
{{- end -}}
