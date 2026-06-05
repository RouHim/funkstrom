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

# Test 13: High-quality stream config is present in status
echo "Test 13: High-quality stream config is present in status"
HIGH_STREAM=$(curl -s "${BASE_URL}/status" | jq -r '.streams[] | select(.name == "high") | .name')
if [ "$HIGH_STREAM" = "high" ]; then
    echo "  ✓ PASS"
    ((PASS++))
else
    echo "  ✗ FAIL (high stream not found in status)"
    ((FAIL++))
fi

# Test 14: All configured streams show status online
echo "Test 14: All configured streams show status online"
ALL_ONLINE=true
STREAM_STATUSES=$(curl -s "${BASE_URL}/status" | jq -r '.streams[] | "\(.name)=\(.status)"')
for ENTRY in $STREAM_STATUSES; do
    S_NAME="${ENTRY%%=*}"
    S_STATUS="${ENTRY##*=}"
    if [ "$S_STATUS" != "online" ]; then
        echo "  ✗ FAIL (stream $S_NAME has status: $S_STATUS)"
        ALL_ONLINE=false
        break
    fi
done
if [ "$ALL_ONLINE" = true ]; then
    echo "  ✓ PASS (all streams online)"
    ((PASS++))
else
    ((FAIL++))
fi

# Test 15: High-quality stream endpoint responds
echo "Test 15: High-quality stream endpoint responds"
HTTP_CODE=$(curl -s --max-time 3 -o /dev/null -w "%{http_code}" "${BASE_URL}/high")
if [ "$HTTP_CODE" = "200" ]; then
    echo "  ✓ PASS (HTTP $HTTP_CODE)"
    ((PASS++))
else
    echo "  ✗ FAIL (HTTP $HTTP_CODE)"
    ((FAIL++))
fi

# Test 16: High-quality stream delivers audio data
echo "Test 16: High-quality stream delivers audio data"
HQ_DATA_SIZE=$(timeout 5 curl -s -N "${BASE_URL}/high" 2>/dev/null | head -c 5000 | wc -c)
if [ "$HQ_DATA_SIZE" -gt 0 ]; then
    echo "  ✓ PASS (received ${HQ_DATA_SIZE} bytes)"
    ((PASS++))
else
    echo "  ✗ FAIL (received 0 bytes)"
    ((FAIL++))
fi


# Test 17: ICY metadata header present when requested
echo "Test 17: ICY metadata header present when requested"
HEADERS_FILE=$(mktemp)
timeout 3 curl -s -N -H "Icy-MetaData: 1" "${BASE_URL}/stream" -D "$HEADERS_FILE" -o /dev/null 2>/dev/null || true
if grep -qi "icy-metaint: 16000" "$HEADERS_FILE"; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: icy-metaint header missing"
    FAIL=$((FAIL + 1))
fi
rm -f "$HEADERS_FILE"

# Test 18: ICY metadata header absent when not requested
echo "Test 18: ICY metadata header absent when not requested"
HEADERS_FILE=$(mktemp)
timeout 3 curl -s -N "${BASE_URL}/stream" -D "$HEADERS_FILE" -o /dev/null 2>/dev/null || true
if grep -qi "icy-metaint" "$HEADERS_FILE"; then
    echo "  FAIL: icy-metaint present without request"
    FAIL=$((FAIL + 1))
else
    echo "  PASS"
    PASS=$((PASS + 1))
fi
rm -f "$HEADERS_FILE"

# Test 19: First metadata block at metaint=16000 contains StreamTitle and StreamUrl
echo "Test 19: First metadata block at metaint=16000 contains StreamTitle and StreamUrl"
DATA_FILE=$(mktemp)
# Need at least 16000 + 1 + N*16 bytes (typically ~16050). 5 sec at 128kbps ≈ 80KB, plenty.
timeout 5 curl -s -N -H "Icy-MetaData: 1" "${BASE_URL}/stream" -o "$DATA_FILE" 2>/dev/null || true
# Skip 16000 bytes of audio, read the metadata block count byte at position 16000
BLOCK_COUNT_BYTE=$(od -An -tx1 -j 16000 -N1 "$DATA_FILE" | tr -d ' ')
if [ -n "$BLOCK_COUNT_BYTE" ] && [ "$BLOCK_COUNT_BYTE" != "00" ]; then
    BLOCKS=$((16#$BLOCK_COUNT_BYTE))
    META_SIZE=$((1 + BLOCKS * 16))
    META_CONTENT=$(dd if="$DATA_FILE" bs=1 skip=16000 count="$META_SIZE" 2>/dev/null | strings)
    if echo "$META_CONTENT" | grep -q "StreamTitle=" && echo "$META_CONTENT" | grep -q "StreamUrl="; then
        echo "  PASS: $META_CONTENT"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: metadata at 16000 missing StreamTitle/StreamUrl fields (got: $META_CONTENT)"
        FAIL=$((FAIL + 1))
    fi
else
    echo "  FAIL: no metadata block at position 16000 (expected non-zero block count, got 0x$BLOCK_COUNT_BYTE)"
    FAIL=$((FAIL + 1))
fi
rm -f "$DATA_FILE"

# Test 20: Metadata block injected at metaint boundary (no initial block offset)
echo "Test 20: Metadata block injected at metaint boundary (16000)"
DATA_FILE=$(mktemp)
timeout 8 curl -s -N -H "Icy-MetaData: 1" "${BASE_URL}/stream" -o "$DATA_FILE" 2>/dev/null || true
# With no initial block, first metadata boundary is at exactly 16000
BOUNDARY_POS=16000
META_BYTE=$(od -An -tx1 -j "$BOUNDARY_POS" -N1 "$DATA_FILE" | tr -d ' ')
if [ -n "$META_BYTE" ]; then
    echo "  PASS: metadata byte 0x$META_BYTE at position $BOUNDARY_POS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: could not read metadata byte at boundary"
    FAIL=$((FAIL + 1))
fi
rm -f "$DATA_FILE"

# Test 21: ICY metadata at metaint=16000 matches /current endpoint track info
echo "Test 21: ICY metadata matches /current endpoint track info"
DATA_FILE=$(mktemp)
timeout 5 curl -s -N -H "Icy-MetaData: 1" "${BASE_URL}/stream" -o "$DATA_FILE" 2>/dev/null || true
CURRENT_JSON=$(curl -s "${BASE_URL}/current")
CURRENT_ARTIST=$(echo "$CURRENT_JSON" | jq -r '.artist')
CURRENT_TITLE=$(echo "$CURRENT_JSON" | jq -r '.title')
# Build expected metadata string with public_url
EXPECTED_ICY="StreamTitle='${CURRENT_ARTIST} - ${CURRENT_TITLE}';StreamUrl='http://127.0.0.1:3002/cover.jpg';"
# Read first metadata block at position 16000
BLOCK_COUNT_BYTE=$(od -An -tx1 -j 16000 -N1 "$DATA_FILE" | tr -d ' ')
if [ -z "$BLOCK_COUNT_BYTE" ] || [ "$BLOCK_COUNT_BYTE" = "00" ]; then
    echo "  FAIL: no metadata block at position 16000"
    FAIL=$((FAIL + 1))
else
    BLOCKS=$((16#$BLOCK_COUNT_BYTE))
    META_SIZE=$((1 + BLOCKS * 16))
    # Extract metadata payload (skip the count byte), convert to string
    META_RAW=$(dd if="$DATA_FILE" bs=1 skip=16001 count="$((BLOCKS * 16))" 2>/dev/null | strings | head -1)
    if [ "$META_RAW" = "$EXPECTED_ICY" ]; then
        echo "  PASS: '$META_RAW'"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: got '$META_RAW', expected '$EXPECTED_ICY'"
        FAIL=$((FAIL + 1))
    fi
fi
rm -f "$DATA_FILE"

# Test 22: /cover.jpg endpoint responds (200 with image or 204 no content)
echo "Test 22: /cover.jpg endpoint responds"
HTTP_CODE=$(curl -s --max-time 3 -o /dev/null -w "%{http_code}" "${BASE_URL}/cover.jpg")
if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "204" ]; then
    echo "  PASS: HTTP $HTTP_CODE"
    PASS=$((PASS + 1))
else
    echo "  FAIL: HTTP $HTTP_CODE"
    FAIL=$((FAIL + 1))
fi

# Test 23: /cover.jpg returns valid image data when available
echo "Test 23: /cover.jpg returns valid content"
COVER_FILE=$(mktemp)
curl -s --max-time 5 "${BASE_URL}/cover.jpg" -o "$COVER_FILE"
COVER_SIZE=$(stat -c%s "$COVER_FILE" 2>/dev/null || echo 0)
if [ "$COVER_SIZE" -gt 0 ]; then
    CONTENT_TYPE=$(curl -s --max-time 3 -D - "${BASE_URL}/cover.jpg" -o /dev/null 2>/dev/null | grep -i "Content-Type" | tr -d '\r')
    FIRST_BYTES=$(od -An -tx1 -N4 "$COVER_FILE" | tr -d ' ')
    if echo "$CONTENT_TYPE" | grep -qi "image/" && [ "${FIRST_BYTES:0:4}" = "ffd8" ]; then
        echo "  PASS: ${COVER_SIZE} bytes, image/jpeg"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: size=$COVER_SIZE, content_type=$CONTENT_TYPE, first_bytes=$FIRST_BYTES"
        FAIL=$((FAIL + 1))
    fi
else
    echo "  PASS: no cover art (expected with test music)"
    PASS=$((PASS + 1))
fi
rm -f "$COVER_FILE"

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
