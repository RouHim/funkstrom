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
STATUS=$(curl -s "${BASE_URL}/status" | jq -r '.status')
if [ "$STATUS" = "online" ]; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL (got: $STATUS)"
    ((FAIL++))
fi

# Test 3: Buffer has data
echo "Test 3: Buffer has data"
BUFFER_CHUNKS=$(curl -s "${BASE_URL}/status" | jq -r '.buffer_chunks')
if [ "$BUFFER_CHUNKS" -gt 0 ]; then
    echo "  ✓ PASS (${BUFFER_CHUNKS} chunks)"
    ((PASS++))
else
    echo "  ✗ FAIL (buffer is empty)"
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
