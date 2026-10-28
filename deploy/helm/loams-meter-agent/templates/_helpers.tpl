{{/* The release's object name. */}}
{{- define "loams-meter-agent.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 50 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 50 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Labels every object carries. */}}
{{- define "loams-meter-agent.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "loams-meter-agent.selector" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "loams-meter-agent.serviceAccount" -}}
{{- default (include "loams-meter-agent.fullname" .) .Values.serviceAccount.name -}}
{{- end -}}

{{- define "loams-meter-agent.image" -}}
{{- printf "%s@%s" .Values.image.repository (required "image.digest: pin the agent image by digest (sha256:...)" .Values.image.digest) -}}
{{- end -}}
