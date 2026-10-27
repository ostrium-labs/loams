{{/* The labels that select one role's pods. Never the version: selectors are immutable. */}}
{{- define "loams-risingwave.selector" -}}
app.kubernetes.io/name: risingwave
app.kubernetes.io/component: {{ .component }}
loams.dev/engine-instance: {{ .instance }}
{{- end -}}

{{/* The labels that select every pod of the instance. */}}
{{- define "loams-risingwave.instanceSelector" -}}
app.kubernetes.io/name: risingwave
loams.dev/engine-instance: {{ .Values.instance }}
{{- end -}}
