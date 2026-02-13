#!/bin/sh
# SPDX-License-Identifier: BUSL-1.1
# Copyright (c) 2026 Alfred Jean LLC
#
# Pod-creation script for coopmux running in Kubernetes.
# Called by coopmux when POST /api/v1/sessions/launch fires.
#
# Expected env (set by coopmux launch handler):
#   COOP_MUX_URL   - mux URL (ignored; we use Service DNS instead)
#   COOP_MUX_TOKEN - auth token for session registration
#
# Expected env (set in coopmux pod spec):
#   POD_NAMESPACE        - namespace (downward API)
#   COOP_SESSION_IMAGE   - image for session pods (default: coop:claude)

set -eu

SESSION_ID=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || uuidgen)
SHORT_ID=$(echo "$SESSION_ID" | cut -c1-8)
POD_NAME="coop-session-${SHORT_ID}"
NAMESPACE="${POD_NAMESPACE:-coop}"
IMAGE="${COOP_SESSION_IMAGE:-coop:claude}"
MUX_URL="http://coopmux.${NAMESPACE}.svc.cluster.local:9800"
MUX_TOKEN="${COOP_MUX_TOKEN:-}"

kubectl apply -n "$NAMESPACE" -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: ${POD_NAME}
  namespace: ${NAMESPACE}
  labels:
    app: coop-session
spec:
  serviceAccountName: coop-session
  restartPolicy: Never
  containers:
    - name: coop
      image: ${IMAGE}
      imagePullPolicy: Never
      command: ["sh", "-c"]
      args:
        - |
          export COOP_URL="http://\${POD_IP}:8080"
          exec coop --host 0.0.0.0 --port 8080 --log-format text -- claude
      env:
        - name: COOP_MUX_URL
          value: "${MUX_URL}"
        - name: COOP_MUX_TOKEN
          value: "${MUX_TOKEN}"
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: POD_NAMESPACE
          valueFrom:
            fieldRef:
              fieldPath: metadata.namespace
        - name: POD_IP
          valueFrom:
            fieldRef:
              fieldPath: status.podIP
        - name: ANTHROPIC_API_KEY
          valueFrom:
            secretKeyRef:
              name: anthropic-credentials
              key: api-key
              optional: true
        - name: CLAUDE_CODE_OAUTH_TOKEN
          valueFrom:
            secretKeyRef:
              name: anthropic-credentials
              key: oauth-token
              optional: true
      ports:
        - containerPort: 8080
      livenessProbe:
        httpGet:
          path: /api/v1/health
          port: 8080
        initialDelaySeconds: 5
        periodSeconds: 10
      readinessProbe:
        httpGet:
          path: /api/v1/health
          port: 8080
        initialDelaySeconds: 2
        periodSeconds: 5
EOF

echo "Created session pod: ${POD_NAME}"
