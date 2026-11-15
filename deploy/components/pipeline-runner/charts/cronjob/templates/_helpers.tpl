{{/*
The prefix every object of this release carries: the project owns them all.
*/}}
{{- define "pipelineCronjob.fullname" -}}
{{- if .Values.project }}
{{- printf "pipeline-%s" .Values.project | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{- define "pipelineCronjob.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/name: {{ include "pipelineCronjob.fullname" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
{{- end }}

{{- define "pipelineCronjob.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "pipelineCronjob.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "pipelineCronjob.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s:%s@%s" .Values.image.repository .Values.image.tag .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository .Values.image.tag }}
{{- end }}
{{- end }}

{{/*
The cadence of one pipeline, as a dict of `schedule`, `interval` and `count` (PL-26, PL-27).

Cron cannot express anything shorter than a minute, so a period under a minute runs every
minute and the Bento trigger fires floor(60 / period) times inside that one pod. A period of a
minute or more either carries its own cron expression or runs every minute with the period as
the trigger interval. This is the same rule as `jcctl::pipelines::cron_job`, which decides the
class for a `kind: Pipeline`; two implementations of PL-27 that disagree would be the bug.
*/}}
{{- define "pipelineCronjob.cadence" -}}
{{- $name := .name }}
{{- $seconds := .pipeline.periodSeconds | default 0 | int }}
{{- $schedule := .pipeline.schedule | default "" }}
{{- if and (gt $seconds 0) (lt $seconds 30) }}
{{- fail (printf "pipeline %q: a period of %ds is faster than the thirty-second rule, so it belongs in the resident streams runner rather than in a CronJob (PL-26)" $name $seconds) }}
{{- end }}
{{- if and (le $seconds 0) (eq $schedule "") }}
{{- fail (printf "pipeline %q: needs either periodSeconds or a cron schedule; without one the pipeline would never be triggered" $name) }}
{{- end }}
{{- if and (gt $seconds 0) (lt $seconds 60) }}
{{- dict "schedule" "* * * * *" "interval" (printf "%ds" $seconds) "count" (div 60 $seconds) | toYaml }}
{{- else if and (gt $seconds 0) (eq $schedule "") }}
{{- dict "schedule" "* * * * *" "interval" (printf "%ds" $seconds) "count" 1 | toYaml }}
{{- else }}
{{- dict "schedule" $schedule "interval" "" "count" 1 | toYaml }}
{{- end }}
{{- end }}
