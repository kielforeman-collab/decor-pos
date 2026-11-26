{{- define "decor-pos.fullname" -}}
{{- printf "%s-%s" .Release.Name "decor-pos" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "decor-pos.edge.fullname" -}}
{{- printf "%s-edge" (include "decor-pos.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "decor-pos.api.fullname" -}}
{{- printf "%s-api" (include "decor-pos.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "decor-pos.postgres.fullname" -}}
{{- printf "%s-postgres" (include "decor-pos.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "decor-pos.labels" -}}
app.kubernetes.io/name: decor-pos
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end -}}

{{- define "decor-pos.edge.dataDir" -}}/data{{- end -}}
