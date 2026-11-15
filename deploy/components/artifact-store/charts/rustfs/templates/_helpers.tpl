{{- define "rustfs.fullname" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "rustfs.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "rustfs.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "rustfs.selectorLabels" -}}
app.kubernetes.io/name: {{ include "rustfs.fullname" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "rustfs.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "rustfs.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "rustfs.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s:%s@%s" .Values.image.repository .Values.image.tag .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository .Values.image.tag }}
{{- end }}
{{- end }}

{{- define "rustfs.bootstrapImage" -}}
{{- if .Values.bootstrapImage.digest }}
{{- printf "%s:%s@%s" .Values.bootstrapImage.repository .Values.bootstrapImage.tag .Values.bootstrapImage.digest }}
{{- else }}
{{- printf "%s:%s" .Values.bootstrapImage.repository .Values.bootstrapImage.tag }}
{{- end }}
{{- end }}

{{/* The S3 endpoint inside the cluster: no ingress route exists and none should (PF-32). */}}
{{- define "rustfs.endpoint" -}}
{{- printf "http://%s.%s.svc.cluster.local:%d" (include "rustfs.fullname" .) .Release.Namespace (int .Values.service.port) }}
{{- end }}
