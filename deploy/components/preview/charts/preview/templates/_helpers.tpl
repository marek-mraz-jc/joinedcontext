{{- define "preview.namespace" -}}
pv-{{ required "slug names the preview: just dev-preview <slug>" .Values.slug }}
{{- end }}

{{- define "preview.host" -}}
preview-{{ .Values.slug }}.{{ .Values.domain }}
{{- end }}

{{- /* pv_{slug}: a Postgres identifier, so the slug's dashes become underscores. */}}
{{- define "preview.db" -}}
pv_{{ .Values.slug | replace "-" "_" }}
{{- end }}

{{- define "preview.labels" -}}
app.kubernetes.io/part-of: preview
app.kubernetes.io/managed-by: dev-preview
sandbox.joinedcontext.com/lifecycle: preview
{{- end }}
