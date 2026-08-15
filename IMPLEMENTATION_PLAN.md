# Fact-layer reliability

Branch: `fact-layer` from origin/main. Signed desktop dogfood after each stage.

## Stage F1a: boot recovery (in progress)

**Goal**: restart leaves sessions and jobs consistent; SQLite waits on a busy peer.

**Success**:
- Open sessions close at last event time (or start time if no events)
- Disabled processors skip their open jobs with a reason
- Enabled workers reclaim stale `running` jobs
- Second boot changes nothing
- App connection `busy_timeout` is 5s

**Status**: Complete

## Stage F1b: restart budget

Shared rolling restart budget across desktop supervisor and health monitor. Intentional config/pause reloads are not crashes. Remove 5-minute write-watchdog suicide as daily recovery.

**Status**: Not Started

## Stage F2–F6

Session/segment contract, screenshot metadata, one persist path, derived-job terminals, signed soak. See research plan (outside this repo).

**Status**: Not Started
