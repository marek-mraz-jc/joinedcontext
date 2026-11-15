{{- define "ckan.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "ckan.fullname" -}}
{{- default (printf "%s-%s" .Release.Name (include "ckan.name" .)) .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "ckan.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/* Labels every object of this chart carries. `part` names the workload. */}}
{{- define "ckan.labels" -}}
helm.sh/chart: {{ include "ckan.chart" .root }}
app.kubernetes.io/name: {{ include "ckan.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .part }}
app.kubernetes.io/version: {{ .root.Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .root.Release.Service }}
{{- end }}

{{- define "ckan.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ckan.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .part }}
{{- end }}

{{/* `repo:tag@digest`, so what runs is the image that was reviewed (OPS-27). */}}
{{- define "ckan.image" -}}
{{- if not .repository -}}
{{- fail "an image of the ckan chart has no repository: set its image.repository and image.digest (components/ckan/images.yaml)" -}}
{{- end -}}
{{- if .digest -}}
{{ .repository }}:{{ .tag }}@{{ .digest }}
{{- else -}}
{{ .repository }}:{{ .tag }}
{{- end -}}
{{- end }}

{{- define "ckan.podAnnotations" -}}
{{- if .Values.serviceMesh.enabled }}
linkerd.io/inject: enabled
{{- end }}
{{- end }}
