{{- define "bootstrap.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "bootstrap.fullname" -}}
{{- $name := include "bootstrap.name" . }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{- define "bootstrap.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "bootstrap.selectorLabels" . }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "bootstrap.selectorLabels" -}}
app.kubernetes.io/name: {{ include "bootstrap.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "bootstrap.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s:%s@%s" .Values.image.repository .Values.image.tag .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository .Values.image.tag }}
{{- end }}
{{- end }}

{{/*
The seed is one ConfigMap per project, `<fullname>-seed-<project>`, beside `<fullname>-seed` for
the organization's own files: one ConfigMap holds at most 1 MiB, and the seed outgrew it when
the fifth city joined (T-3177). `bootstrap.seedGroup` names the group of one `__` key.
*/}}
{{- define "bootstrap.seedGroup" -}}
{{- $parts := splitList "__" . -}}
{{- if and (eq (index $parts 0) "projects") (gt (len $parts) 2) }}{{ index $parts 1 }}{{ end -}}
{{- end }}

{{- define "bootstrap.seedGroups" -}}
{{- $groups := dict -}}
{{- range $key, $text := .Values.seed }}{{ $_ := set $groups (include "bootstrap.seedGroup" $key) true }}{{ end -}}
{{- keys $groups | sortAlpha | toJson -}}
{{- end }}
