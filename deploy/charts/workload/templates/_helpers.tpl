{{/*
Expand the name of the chart.
*/}}
{{- define "workload.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "workload.fullname" -}}
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
{{- define "workload.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "workload.labels" -}}
helm.sh/chart: {{ include "workload.chart" . }}
{{ include "workload.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "workload.selectorLabels" -}}
app.kubernetes.io/name: {{ include "workload.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "workload.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "workload.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the image reference: repo:tag@digest or repo:tag
*/}}
{{- define "workload.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s:%s@%s" .Values.image.repository .Values.image.tag .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository .Values.image.tag }}
{{- end }}
{{- end }}

{{/*
GOMEMLIMIT for a Go workload: `goMemLimitPercent` of the container's memory limit, in bytes,
read from the merged `resources.limits.memory` so an environment that raises the limit raises
this with it (T-3537). Kubernetes binary and decimal suffixes; anything else fails the render.
*/}}
{{- define "workload.goMemLimit" -}}
{{- $limit := toString .Values.resources.limits.memory -}}
{{- $number := regexFind "^[0-9]+" $limit -}}
{{- $suffix := trimPrefix $number $limit -}}
{{- $units := dict "" 1 "Ki" 1024 "Mi" 1048576 "Gi" 1073741824 "k" 1000 "M" 1000000 "G" 1000000000 -}}
{{- if or (not $number) (not (hasKey $units $suffix)) -}}
{{- fail (printf "goMemLimitPercent: resources.limits.memory %q is not a whole number with a Ki/Mi/Gi/k/M/G suffix" $limit) -}}
{{- end -}}
{{- $percent := int .Values.goMemLimitPercent -}}
{{- if or (lt $percent 1) (gt $percent 100) -}}
{{- fail (printf "goMemLimitPercent %d is not between 1 and 100" $percent) -}}
{{- end -}}
{{- div (mul (atoi $number) (get $units $suffix) $percent) 100 -}}
{{- end }}
