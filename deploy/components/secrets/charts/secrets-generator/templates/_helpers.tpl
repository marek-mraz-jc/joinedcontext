{{/*
Expand the name of the chart.
*/}}
{{- define "secrets.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "secrets.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "secrets.labels" -}}
helm.sh/chart: {{ include "secrets.chart" . }}
{{ include "secrets.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Common annotations
*/}}
{{- define "secrets.annotations" -}}
meta.helm.sh/release-name: {{ .Release.Name }}
meta.helm.sh/release-namespace: {{ .Release.Namespace }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "secrets.selectorLabels" -}}
app.kubernetes.io/name: {{ include "secrets.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Generate secret data key-value pairs
Usage: {{ include "secrets.generateData" (dict "name" "my-secret" "namespaces" $namespaces "keys" $keys) }}
Returns a YAML map of key-value pairs
*/}}
{{- define "secrets.generateData" -}}
{{- $name := .name -}}
{{- $namespaces := .namespaces | default list -}}
{{- $keys := .keys -}}
{{- /* Check all namespaces for existing secret */ -}}
{{- $existingSecret := dict }}
{{- range $ns := $namespaces }}
  {{- $foundSecret := lookup "v1" "Secret" $ns $name }}
  {{- if $foundSecret }}
    {{- $existingSecret = $foundSecret }}
    {{- break }}
  {{- end }}
{{- end }}
{{- /* Generate data for each key */ -}}
{{- range $key, $value := $keys }}
  {{- if and $existingSecret (hasKey $existingSecret.data $key) }}
{{ $key }}: {{ index $existingSecret.data $key }}
  {{- else }}
    {{- if kindIs "map" $value }}
      {{- if eq (index $value "encoding" | default "") "hex" }}
{{ $key }}: {{ printf "%x" (b64dec (randBytes (int (div ($value.length | int) 2)))) | b64enc | quote }}
      {{- else if index $value "special" }}
        {{- /* Guarantee at least one char of each class required:
             uppercase, lowercase, digit, special. */ -}}
        {{- $safeSpecials := "*+,-.^_~" }}
        {{- $specialIdx := mod (randNumeric 3 | int) (len $safeSpecials) | int }}
        {{- $specialChar := substr $specialIdx (int (add $specialIdx 1)) $safeSpecials }}
        {{- $totalLen := $value.length | int }}
        {{- $bulkLen := int (sub $totalLen 1) }}
        {{- $bulk := printf "%s%s%s%s" (randAlpha 1 | upper) (randAlpha 1 | lower) (randNumeric 1) (randAlphaNum (int (sub $totalLen 4))) }}
        {{- $insertAt := mod (randNumeric 2 | int) $bulkLen | int }}
        {{- $part1 := substr 0 $insertAt $bulk }}
        {{- $part2 := substr $insertAt $bulkLen $bulk }}
        {{- $generated := printf "%s%s%s" $part1 $specialChar $part2 }}
{{ $key }}: {{ $generated | b64enc | quote }}
      {{- else }}
{{ $key }}: {{ randAlphaNum ($value.length | int) | b64enc | quote }}
      {{- end }}
    {{- else if kindIs "float64" $value }}
{{ $key }}: {{ randAlphaNum ($value | int) | b64enc | quote }}
    {{- else }}
{{ $key }}: {{ $value | b64enc | quote }}
    {{- end }}
  {{- end }}
{{- end }}
{{- end }}
