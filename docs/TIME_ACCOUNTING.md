# Activity and time accounting contract

Contract version: `lumen.activity-accounting.v1`. API version remains 1; database
schema 12 adds replay metadata. Observe remains independent of OCR and Act.

## Facts, clocks and identity

`activity.focus.v1` retains its existing payload fields (`app_name`, `bundle_id`,
`window_title`, `url`, `ls_category_type`, `idle_seconds`, `is_idle`, `is_locked`,
`heartbeat`). New producers also send `source_instance_id` (capture-lifetime UUID),
`source_seq` (positive, increasing per emitted focus sample), `window_id` and `pid`.
`SourceEvent.ts` is the UTC capture time, not receipt time. The store records both
capture and receipt milliseconds. All new interval endpoints are normalized to
millisecond precision before projection. Window identity includes process and window ID;
a source restart, privacy discontinuity or backward clock change begins a new
source lifetime. A numeric window ID is never a permanent global identity.

Schema 12 interns repeated payload identities in `activity_sample_identities`.
`activity_samples` stores event ID, source lifetime/sequence, capture/receipt time
and identity reference. Heartbeats do not create screenshot artifacts or flood
the general `events` table. Changed-focus events still use the existing event
path. Exact sample replays are idempotent; conflicting reuse of event or source
sequence identity is rejected. Each sample materializes its interval to the next consecutive sample in
`activity_segments`. Ordered and late arrivals update at most two intervals (the
new sample and its predecessor); ingestion never rebuilds a whole lifetime.
Reads merge adjacent equal identities after releasing the database lock.
Day/range reads use an indexed time window, with one database read per range.

Legacy input without lifetime/sequence remains accepted. Its existing projection
is preserved. Previously discarded legacy heartbeats cannot be reconstructed;
the new replay guarantee starts with schema-12 samples, not invented history.

## Intervals and gates

Stored times are UTC; intervals are half-open `[start,end)`. A segment extends
only from the immediately preceding automatic sample in the same lifetime, with
consecutive sequence numbers and a nonnegative gap under 30 seconds. A→B→A
creates three intervals. A crash, missing sequence, long gap or new lifetime does
not extend the last window to restart time or to the current clock.

Lock observations have no app, title, URL, process or window metadata. Locked
intervals are idle. Pause, closed eyes and blocklisted applications emit no
content-bearing activity and break continuity before resuming. The last accepted
sample remains the end of confirmed time; an unobserved tail is not fabricated.
Idle uses the configured threshold and existing display-sleep-prevention rule.
An unavailable frontmost identity is unknown, not productive or distracting work.
Unknown lock state blocks capture without fabricating a locked-time fact; an
unavailable idle probe breaks continuity instead of being interpreted as activity.

## Manual overlays and calendar aggregation

The raw automatic rows remain intact. Manual entry requires a non-empty app and
`end > start`; overlapping manual entries are rejected. Retrying the identical
entry is idempotent. For historical conflicting manual rows, later start wins,
then stable segment ID; the result is deterministic.

The timeline, scene/history fold, daily totals and date-range totals share an
interval sweep: `manual_union + (auto_union minus manual_union)`. Within legacy
automatic overlaps, later start and stable ID break ties. Deleting a manual entry
reveals the original automatic intervals. Split automatic view rows have stable
`@<start_ms>` suffixes; manual view IDs remain deletable stored IDs.

The read projection clips at actual local calendar-day boundaries, including
23- and 25-hour days. A manual entry may span midnight. Hour buckets allocate the
actual UTC duration to local hours; during fall-back, the repeated hour shares
one of the existing 24 display buckets and can contain two hours of time.
Daily, hourly, category and range totals use the same effective intervals.
Pulse is 0–100 over classified active duration; unknown and missing time are
excluded from its denominator. Range queries accept 1–367 inclusive civil dates.

Internal unobserved spans appear as `source: "gap"` with no app or content and
`is_idle: false`. They are neither work nor idle and do not become narrative or
scene evidence. Times before the first and after the last observed interval are
not inferred. Midnight card persistence also covers the previous local day so
the last closed cards are not lost at the day boundary.

## Rules, APIs and platform ports

Existing `/v1/activity/{segments,stats,range,rules,segment}` routes and DTO fields
remain compatible. The additive `gap` source value lets the timeline explain
missing records. Old DTOs without optional URL/source/scene fields still decode.
Manual delete accepts only stored manual IDs; automatic facts cannot be edited
through this route.

CategoryRule JSON and the existing rule precedence remain unchanged. Saving
rules increments `activity.category_rules_revision`, reclassifies automatic
rows using app/window/URL fields, and leaves source samples and manual category
snapshots untouched. This keeps domain rules valid when reapplying them.
`IdleProbe` and `FrontmostAppProbe` remain platform ports; time accounting has no
screen-recording or native OCR dependency. Liveness frames never enter this
accounting input.

## Validation

Behavior tests cover ordered/late/duplicate samples, A→B→A, source restarts,
window identity changes, privacy discontinuities, backward clocks, manual
masking/conflict/undo, clipped intervals, daily/range consistency, hour sums and
spring/fall DST days. DST tests use a separate process timezone to avoid racing
other tests. API tests cover legacy decoding. Physical permission transitions,
long-running capture and human time reconciliation require separate runtime
acceptance; unit tests do not establish those results.
