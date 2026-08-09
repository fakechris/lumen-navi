#!/usr/bin/env bash
# E2E test for the activity tracking API against a running daemon.
# Requires: daemon running on 127.0.0.1:7420 with at least 5 min of activity data.
#
# Usage: bash scripts/e2e-activity.sh
set -euo pipefail

BASE="http://127.0.0.1:7420"
TODAY=$(date +%Y-%m-%d)
WEEK_AGO=$(date -v-7d +%Y-%m-%d 2>/dev/null || date -d "-7 days" +%Y-%m-%d)
PASS=0
FAIL=0

assert() {
  local name="$1" condition="$2"
  if eval "$condition"; then
    echo "  ✅ $name"
    PASS=$((PASS + 1))
  else
    echo "  ❌ $name (condition: $condition)"
    FAIL=$((FAIL + 1))
  fi
}

echo "=== Activity API E2E Tests (daemon: $BASE, day: $TODAY) ==="
echo ""

# --- Health ---
echo "--- Health ---"
HEALTH=$(curl -sf --max-time 5 "$BASE/v1/health" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin)))" 2>/dev/null) || HEALTH=""
assert "health responds" "[ -n '$HEALTH' ]"
SCHEMA=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('schema_version',0))" 2>/dev/null || echo 0)
assert "schema >= 8" "[ '$SCHEMA' -ge 8 ]"
EVENTS=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('stored_events',0))" 2>/dev/null || echo 0)
assert "has events" "[ '$EVENTS' -gt 0 ]"

# --- Segments ---
echo ""
echo "--- Segments (today) ---"
SEGS=$(curl -sf --max-time 5 "$BASE/v1/activity/segments?day=$TODAY" 2>/dev/null) || SEGS="[]"
SEG_COUNT=$(echo "$SEGS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
assert "segments non-empty" "[ '$SEG_COUNT' -gt 0 ]"

# Check segment fields
echo "$SEGS" | python3 -c "
import sys, json
segs = json.load(sys.stdin)
required = {'seg_id','day','app_name','started_at','ended_at','duration_ms','is_idle','category','productivity_level','event_count','source'}
ok = True
for s in segs[:3]:
    missing = required - set(s.keys())
    if missing:
        print(f'  ❌ segment missing fields: {missing}')
        ok = False
        break
if ok:
    print('  ✅ segment fields complete')
" 2>/dev/null

# Check at least one segment has a category
HAS_CAT=$(echo "$SEGS" | python3 -c "import sys,json; print(any(s.get('category') for s in json.load(sys.stdin)))" 2>/dev/null || echo False)
assert "at least one categorized segment" "[ '$HAS_CAT' = 'True' ]"

# --- Stats (today) ---
echo ""
echo "--- Stats (today) ---"
STATS=$(curl -sf --max-time 5 "$BASE/v1/activity/stats?day=$TODAY" 2>/dev/null) || STATS="{}"
assert "stats responds" "[ '$STATS' != '{}' ]"

ACTIVE_MS=$(echo "$STATS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('total_active_ms',0))" 2>/dev/null || echo 0)
assert "total_active_ms > 0" "[ '$ACTIVE_MS' -gt 0 ]"

# by_hour should be array of 24
HOUR_LEN=$(echo "$STATS" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('by_hour',[])))" 2>/dev/null || echo 0)
assert "by_hour has 24 entries" "[ '$HOUR_LEN' -eq 24 ]"

# top_apps should have app_name field (not garbled concat)
echo "$STATS" | python3 -c "
import sys, json
d = json.load(sys.stdin)
apps = d.get('top_apps', [])
ok = True
for a in apps:
    name = a.get('app_name', '')
    if ',' in name:
        print(f'  ❌ app_name looks like concat: {name}')
        ok = False
        break
if ok and apps:
    print(f'  ✅ top_apps clean ({len(apps)} apps, top: {apps[0][\"app_name\"]})')
elif not apps:
    print('  ⚠️  top_apps empty')
" 2>/dev/null

# --- Range (week) ---
echo ""
echo "--- Range (week: $WEEK_AGO to $TODAY) ---"
RANGE=$(curl -sf --max-time 5 "$BASE/v1/activity/range?from=$WEEK_AGO&to=$TODAY" 2>/dev/null) || RANGE="{}"
assert "range responds" "[ '$RANGE' != '{}' ]"

DAYS_LEN=$(echo "$RANGE" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('days',[])))" 2>/dev/null || echo 0)
assert "range has day rollups" "[ '$DAYS_LEN' -gt 0 ]"

# Range top_apps also clean
echo "$RANGE" | python3 -c "
import sys, json
d = json.load(sys.stdin)
apps = d.get('top_apps', [])
ok = True
for a in apps:
    name = a.get('app_name', '')
    if ',' in name:
        print(f'  ❌ range app_name concat: {name}')
        ok = False
        break
if ok and apps:
    print(f'  ✅ range top_apps clean ({len(apps)} apps)')
" 2>/dev/null

# --- Category rules CRUD ---
echo ""
echo "--- Category Rules CRUD ---"
RULES_BEFORE=$(curl -sf --max-time 5 "$BASE/v1/activity/rules" 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)

# Add a test rule
curl -sf -X POST "$BASE/v1/activity/rules" -H "Content-Type: application/json" \
  -d '[{"field":"app_name","value":"E2E_TEST_APP","category":"Testing","level":"neutral"}]' >/dev/null 2>&1
RULES_AFTER=$(curl -sf --max-time 5 "$BASE/v1/activity/rules" 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
assert "rule added" "[ '$RULES_AFTER' -gt '$RULES_BEFORE' ]"

# Verify the rule is there
HAS_RULE=$(curl -sf --max-time 5 "$BASE/v1/activity/rules" 2>/dev/null | python3 -c "
import sys,json
rules=json.load(sys.stdin)
print(any(r.get('value')=='E2E_TEST_APP' for r in rules))
" 2>/dev/null || echo False)
assert "rule content correct" "[ '$HAS_RULE' = 'True' ]"

# Remove the test rule (rewrite without it)
curl -sf --max-time 5 "$BASE/v1/activity/rules" | python3 -c "
import sys,json
rules=json.load(sys.stdin)
filtered=[r for r in rules if r.get('value')!='E2E_TEST_APP']
print(json.dumps(filtered))
" 2>/dev/null | curl -sf -X POST "$BASE/v1/activity/rules" -H "Content-Type: application/json" -d @- >/dev/null 2>&1
RULES_FINAL=$(curl -sf --max-time 5 "$BASE/v1/activity/rules" 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
assert "rule removed" "[ '$RULES_FINAL' -eq '$RULES_BEFORE' ]"

# --- Manual segment CRUD ---
echo ""
echo "--- Manual Segment CRUD ---"
# Use local timezone offset (not hardcoded +08:00 — machine may be in PDT)
LOCAL_OFFSET=$(date +%z | sed 's/\([+-][0-9][0-9]\)\([0-9][0-9]\)/\1:\2/')
START_TS="${TODAY}T14:00:00${LOCAL_OFFSET}"
END_TS="${TODAY}T14:30:00${LOCAL_OFFSET}"
SEG_RESP=$(curl -sf -X POST "$BASE/v1/activity/segment" -H "Content-Type: application/json" \
  -d "{\"started_at\":\"$START_TS\",\"ended_at\":\"$END_TS\",\"app_name\":\"E2E_MEETING\",\"category\":\"Communication\",\"productivity_level\":\"neutral\"}" 2>/dev/null) || SEG_RESP=""
SEG_ID=$(echo "$SEG_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('seg_id',''))" 2>/dev/null || echo "")
assert "manual segment added" "[ -n '$SEG_ID' ]"

# Verify it shows in segments
HAS_MANUAL=$(curl -sf --max-time 5 "$BASE/v1/activity/segments?day=$TODAY" 2>/dev/null | python3 -c "
import sys,json
segs=json.load(sys.stdin)
print(any(s.get('app_name')=='E2E_MEETING' and s.get('source')=='manual' for s in segs))
" 2>/dev/null || echo False)
assert "manual segment visible" "[ '$HAS_MANUAL' = 'True' ]"

# Delete it
DEL_RESP=$(curl -sf -X DELETE "$BASE/v1/activity/segment?seg_id=$SEG_ID" 2>/dev/null) || DEL_RESP=""
DEL_OK=$(echo "$DEL_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('ok',False))" 2>/dev/null || echo False)
assert "manual segment deleted" "[ '$DEL_OK' = 'True' ]"

# Verify gone
STILL_THERE=$(curl -sf --max-time 5 "$BASE/v1/activity/segments?day=$TODAY" 2>/dev/null | python3 -c "
import sys,json
segs=json.load(sys.stdin)
print(any(s.get('app_name')=='E2E_MEETING' for s in segs))
" 2>/dev/null || echo True)
assert "manual segment gone" "[ '$STILL_THERE' = 'False' ]"

# --- Summary ---
echo ""
echo "================================"
echo "  PASS: $PASS  FAIL: $FAIL"
echo "================================"
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
