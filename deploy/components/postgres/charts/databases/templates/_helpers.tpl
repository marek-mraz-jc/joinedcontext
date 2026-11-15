{{/*
Expand the name of the chart.
*/}}
{{- define "database.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "database.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "database.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "database.labels" -}}
helm.sh/chart: {{ include "database.chart" . }}
{{ include "database.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "database.selectorLabels" -}}
app.kubernetes.io/name: {{ include "database.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "database.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "database.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
The pod securityContext of the two pods that mount the local-backup volume: the dump Job and the
drill. On an SELinux node each pod relabels a local-path volume to its own random MCS categories
on mount, so the drill could not open what the dump wrote ("Permission denied" on a 0777 dir,
prod 2026-11-01, T-6298). One fixed pair for both, still container_t.
*/}}
{{- define "database.localBackupPodSecurityContext" -}}
{{- toYaml (merge (dict "seLinuxOptions" (dict "level" "s0:c217,c602")) (deepCopy .Values.securityDefaults.podSecurityContext)) }}
{{- end }}
