#!/bin/bash
# Basic E2E tests for Funkstrom

BASE_URL="http://127.0.0.1:3002"
PASS=0
FAIL=0

echo "=== Funkstrom E2E Tests ==="
echo

# Check if server is running
if ! curl -s --max-time 2 "${BASE_URL}/status" > /dev/null 2>&1; then
    echo "✗ ERROR: Server is not running at $BASE_URL"
    echo "Please start the server first: cargo run -- --config config.toml"
    exit 1
fi

# Test 1: Status endpoint returns valid JSON
echo "Test 1: Status endpoint returns valid JSON"
if curl -s "${BASE_URL}/status" | jq . > /dev/null 2>&1; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL"
    ((FAIL++))
fi

# Test 2: Status shows server is online
echo "Test 2: Server status is online"
STATUS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].status')
if [ "$STATUS" = "online" ]; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL (got: $STATUS)"
    ((FAIL++))
fi

# Test 3: Buffer behavior matches listener count
echo "Test 3: Buffer behavior matches listener count"
STATUS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].status')
BUFFER_CHUNKS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].buffer_chunks')
LISTENERS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].listeners')
if [ "$STATUS" = "online" ] || [ "$STATUS" = "idle" ]; then
    if [ "$LISTENERS" -gt 0 ] && [ "$BUFFER_CHUNKS" -eq 0 ]; then
        # With listeners, buffer should have data
        echo "  ✗ FAIL (listeners: $LISTENERS but buffer empty)"
        ((FAIL++))
    else
        # 0 listeners: buffer empty is OK; 1+ listeners: buffer should have data
        echo "  ✓ PASS (status: $STATUS, listeners: $LISTENERS, buffer: ${BUFFER_CHUNKS} chunks)"
        ((PASS++))
    fi
else
    echo "  ✗ FAIL (unexpected status: $STATUS)"
    ((FAIL++))
fi

# Test 4: Stream returns Icecast headers
echo "Test 4: Stream returns Icecast headers"
HEADERS_FILE=$(mktemp)
timeout 3 curl -s -N "${BASE_URL}/stream" -D "$HEADERS_FILE" -o /dev/null 2>/dev/null || true
if grep -qi "icy-name" "$HEADERS_FILE"; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL"
    ((FAIL++))
fi
rm -f "$HEADERS_FILE"

# Test 5: Stream sends audio data
echo "Test 5: Stream sends audio data"
DATA_SIZE=$(timeout 3 curl -s -N "${BASE_URL}/stream" 2>/dev/null | head -c 10000 | wc -c)
if [ "$DATA_SIZE" -gt 5000 ]; then
    echo "  ✓ PASS (received ${DATA_SIZE} bytes)"
    ((PASS++))
else
    echo "  ✗ FAIL (only ${DATA_SIZE} bytes)"
    ((FAIL++))
fi

# Test 6: Info page is accessible
echo "Test 6: Info page is accessible"
if curl -s "${BASE_URL}/" | grep -q "Funkstrom"; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL"
    ((FAIL++))
fi

# Test 7: Current track endpoint returns valid JSON
echo "Test 7: Current track endpoint returns valid JSON"
if curl -s "${BASE_URL}/current" | jq . > /dev/null 2>&1; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL"
    ((FAIL++))
fi

# Test 8: Current track has required metadata fields
echo "Test 8: Current track has required metadata fields"
METADATA=$(curl -s "${BASE_URL}/current")
HAS_TITLE=$(echo "$METADATA" | jq -r '.title' | grep -v "null" | wc -l)
HAS_ARTIST=$(echo "$METADATA" | jq -r '.artist' | grep -v "null" | wc -l)
HAS_ALBUM=$(echo "$METADATA" | jq -r '.album' | grep -v "null" | wc -l)
HAS_PATH=$(echo "$METADATA" | jq -r '.file_path' | grep -v "null" | wc -l)

if [ "$HAS_TITLE" -eq 1 ] && [ "$HAS_ARTIST" -eq 1 ] && [ "$HAS_ALBUM" -eq 1 ] && [ "$HAS_PATH" -eq 1 ]; then
    TITLE=$(echo "$METADATA" | jq -r '.title')
    ARTIST=$(echo "$METADATA" | jq -r '.artist')
    echo "  ✓ PASS (Now Playing: $ARTIST - $TITLE)"
    ((PASS++))
else
    echo "  ✗ FAIL (missing metadata fields)"
    ((FAIL++))
fi

# Test 9: Info page displays current track
echo "Test 9: Info page displays current track"
if curl -s "${BASE_URL}/" | grep -q "Now Playing"; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL"
    ((FAIL++))
fi

sleep 1

# Test 10: Status shows listener count (zero listeners at start)
echo "Test 10: Status shows listener count"
LISTENERS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].listeners')
if [ "$LISTENERS" = "0" ]; then
    echo "  ✓ PASS (listeners: $LISTENERS)"
    ((PASS++))
else
    echo "  ✗ FAIL (expected 0, got: $LISTENERS)"
    ((FAIL++))
fi

# Test 11: Listener count increments on connection
echo "Test 11: Listener count increments on connection"
STREAM_NAME=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].name')
curl -s -N "${BASE_URL}/${STREAM_NAME}" > /dev/null 2>&1 &
CURL_PID=$!
sleep 1
LISTENERS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].listeners')
if [ "$LISTENERS" -ge 1 ]; then
    echo "  ✓ PASS (listeners: $LISTENERS)"
    ((PASS++))
else
    echo "  ✗ FAIL (expected >= 1, got: $LISTENERS)"
    ((FAIL++))
fi
kill $CURL_PID 2>/dev/null
wait $CURL_PID 2>/dev/null

# Test 12: Listener count decrements on disconnect
echo "Test 12: Listener count decrements on disconnect"
sleep 2
LISTENERS=$(curl -s "${BASE_URL}/status" | jq -r '.streams[0].listeners')
if [ "$LISTENERS" = "0" ]; then
    echo "  ✓ PASS (listeners: $LISTENERS)"
    ((PASS++))
else
    echo "  ✗ FAIL (expected 0, got: $LISTENERS)"
    ((FAIL++))
fi

echo
echo "=== Results ==="
echo "Passed: $PASS"
echo "Failed: $FAIL"
echo "Total:  $((PASS + FAIL))"
echo

if [ $FAIL -eq 0 ]; then
    echo "✓ All tests passed!"
    exit 0
else
    echo "✗ Some tests failed"
    exit 1
fi
