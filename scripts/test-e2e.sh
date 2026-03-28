#!/bin/bash
set -e

echo "=== Prerequisites ==="
command -v podman >/dev/null || { echo "podman not found"; exit 1; }
command -v curl >/dev/null || { echo "curl not found"; exit 1; }
[ -n "$OPENAI_API_KEY" ] || { echo "OPENAI_API_KEY not set"; exit 1; }

echo "=== Building ==="
cargo build --workspace --release

echo "=== Building test image ==="
cp target/release/nac images/nac
podman build -t nac:base -f images/Dockerfile.base images/
rm images/nac

echo "=== Starting nacserver ==="
NAC_PORT=3123 ./target/release/nacserver &
SERVER_PID=$!
sleep 2
trap "kill $SERVER_PID 2>/dev/null; exit" EXIT

echo "=== Health check ==="
curl -sf http://localhost:3123/health | jq .

echo "=== Creating task ==="
TASK_ID=$(curl -sf -X POST http://localhost:3123/tasks \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Create a file called hello.txt containing hello from nac", "image": "nac:base"}' | jq -r '.task_id')
echo "Task: $TASK_ID"

echo "=== Polling for completion ==="
for i in $(seq 1 60); do
  STATUS=$(curl -sf "http://localhost:3123/tasks/$TASK_ID" | jq -r '.status')
  echo "  [$i] status: $STATUS"
  if [ "$STATUS" = "completed" ] || [ "$STATUS" = "failed" ]; then
    break
  fi
  sleep 5
done

echo "=== Task result ==="
curl -sf "http://localhost:3123/tasks/$TASK_ID" | jq .

echo "=== Follow-up task ==="
TASK_ID2=$(curl -sf -X POST http://localhost:3123/tasks \
  -H "Content-Type: application/json" \
  -d "{\"prompt\": \"Read hello.txt and tell me what it says\", \"image\": \"nac:base\", \"parent_task_id\": \"$TASK_ID\"}" | jq -r '.task_id')
echo "Follow-up task: $TASK_ID2"

for i in $(seq 1 60); do
  STATUS=$(curl -sf "http://localhost:3123/tasks/$TASK_ID2" | jq -r '.status')
  echo "  [$i] status: $STATUS"
  if [ "$STATUS" = "completed" ] || [ "$STATUS" = "failed" ]; then
    break
  fi
  sleep 5
done

echo "=== Follow-up result ==="
curl -sf "http://localhost:3123/tasks/$TASK_ID2" | jq .

echo "=== PASS ==="
