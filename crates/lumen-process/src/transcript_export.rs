//! Session transcript export → `lumen-transcript.v1` interchange documents.
//!
//! Aggregates the per-chunk `transcript.v1` derived docs of one audio session
//! (see [`crate::DERIVED_TRANSCRIPT_V1`]) into a single shared
//! [`lumen_transcript::TranscriptV1`] document that lumen-cut can import
//! ("Import from Navi"). Mapping per `lumen-suite/contracts/TRANSCRIPT.md` §2.1/§5.
//!
//! ## Time synthesis (contract §4 #1)
//!
//! Navi's derived transcript docs carry no timing; it lives on the chunk
//! events. Decisions made here:
//!
//! - **Session origin**: navi never persists an `audio_session.v1` start
//!   event, so the origin is the earliest chunk event `ts` of the session —
//!   the first segment always starts at `0.0`.
//! - **Chunk start** = chunk event `ts` − origin, in seconds.
//!   **Duration** = `audio_bytes / (sample_rate × 2)` (mono s16le WAV bytes);
//!   falls back to the payload `duration_ms` when `audio_bytes` is absent.
//! - **Out-of-order rows**: chunks are sorted by `(ts, session_ordinal)`
//!   before synthesis, so a lagging insert cannot produce a negative or
//!   backwards timeline.
//! - **No overlaps**: if a computed start lands before the previous segment's
//!   end (timestamp jitter between chunk boundaries), the start is clamped to
//!   that end; the duration is preserved.
//! - **Missing chunks** (silence-dropped, ASR pending/failed): the timeline
//!   keeps a hole — later segments are *not* pulled forward.

use chrono::{DateTime, Utc};
use lumen_store::{SessionDerivedRow, SqliteStore};
use lumen_transcript::{Media, Provenance, Segment, TranscriptV1};
use lumen_types::event_kind;
use serde_json::{Map, Value};
use thiserror::Error;
use uuid::Uuid;

use crate::DERIVED_TRANSCRIPT_V1;

/// Assumed when a chunk payload lacks `sample_rate` (navi captures 16 kHz mono).
const DEFAULT_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Error)]
pub enum TranscriptExportError {
    #[error("store: {0}")]
    Store(#[from] lumen_store::StoreError),
    #[error("session {0} has no transcribed audio chunks")]
    EmptySession(Uuid),
}

/// One transcribed audio chunk: event timing + `transcript.v1` body fields.
#[derive(Debug, Clone)]
struct ChunkTranscript {
    event_id: Uuid,
    ts: DateTime<Utc>,
    /// `session_ordinal` from the chunk payload; tie-breaker for equal `ts`.
    ordinal: u64,
    sample_rate: u32,
    /// WAV byte size recorded in the derived doc (`audio_bytes`).
    audio_bytes: Option<u64>,
    /// Payload `duration_ms`; duration fallback when `audio_bytes` is absent.
    payload_duration_ms: Option<u64>,
    text: String,
    confidence: Option<f64>,
    language: Option<String>,
    engine: Option<String>,
}

impl ChunkTranscript {
    fn from_row(row: &SessionDerivedRow) -> Self {
        let body: Value = serde_json::from_str(&row.body).unwrap_or(Value::Null);
        let payload = &row.payload;
        Self {
            event_id: row.event_id,
            ts: row.ts,
            ordinal: payload
                .get("session_ordinal")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            sample_rate: payload
                .get("sample_rate")
                .and_then(Value::as_u64)
                .map(|r| r as u32)
                .filter(|r| *r > 0)
                .unwrap_or(DEFAULT_SAMPLE_RATE),
            audio_bytes: body.get("audio_bytes").and_then(Value::as_u64),
            payload_duration_ms: payload.get("duration_ms").and_then(Value::as_u64),
            text: body
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            confidence: body.get("confidence").and_then(Value::as_f64),
            language: body
                .get("language")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            engine: body
                .get("engine")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }
    }

    /// Chunk audio duration in seconds (contract: `audio_bytes / (rate × 2)`).
    fn duration_seconds(&self) -> f64 {
        match self.audio_bytes {
            Some(bytes) => bytes as f64 / (f64::from(self.sample_rate) * 2.0),
            None => self.payload_duration_ms.unwrap_or(0) as f64 / 1000.0,
        }
    }
}

/// Export one audio session's transcripts as a `lumen-transcript.v1` document.
///
/// Fails with [`TranscriptExportError::EmptySession`] when the session has no
/// chunk with a `transcript.v1` derived doc (unknown session id, all-silence
/// session, or ASR not finished yet).
pub fn export_session_transcript(
    store: &SqliteStore,
    session_id: Uuid,
) -> Result<TranscriptV1, TranscriptExportError> {
    let rows = store.list_session_derived(
        session_id,
        event_kind::AUDIO_CHUNK_V1,
        DERIVED_TRANSCRIPT_V1,
    )?;
    let chunks: Vec<ChunkTranscript> = rows.iter().map(ChunkTranscript::from_row).collect();
    if chunks.is_empty() {
        return Err(TranscriptExportError::EmptySession(session_id));
    }
    Ok(build_document(chunks))
}

fn build_document(mut chunks: Vec<ChunkTranscript>) -> TranscriptV1 {
    chunks.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.ordinal.cmp(&b.ordinal)));
    let origin = chunks[0].ts;

    // Per-session rollups.
    let uniform_language = uniform(chunks.iter().map(|c| c.language.clone()));
    let uniform_sample_rate = uniform(chunks.iter().map(|c| Some(c.sample_rate)));
    let (majority_engine, engine_counts) = majority_engine(&chunks);
    let total_bytes: u64 = chunks.iter().filter_map(|c| c.audio_bytes).sum();

    let mut segments = Vec::with_capacity(chunks.len());
    let mut cursor = 0.0f64;
    for chunk in &chunks {
        let raw_start = (chunk.ts - origin)
            .to_std()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        // Never overlap the previous segment; keep holes where chunks are missing.
        let start = raw_start.max(cursor);
        let end = start + chunk.duration_seconds();
        cursor = end;

        let mut segment = Segment::new(start, end, chunk.text.clone())
            .with_id(chunk.event_id.to_string());
        if let Some(c) = chunk.confidence {
            segment = segment.with_confidence(c);
        }
        // Language goes to provenance when uniform; per-segment otherwise.
        if uniform_language.is_none() {
            if let Some(lang) = &chunk.language {
                segment = segment.with_language(lang.clone());
            }
        }
        segments.push(segment);
    }

    let mut provenance = Provenance::new("lumen-navi");
    provenance.app_version = Some(env!("CARGO_PKG_VERSION").to_string());
    provenance.engine = majority_engine;
    provenance.language = uniform_language;
    provenance.created_at = Some(Utc::now().to_rfc3339());
    if let Some(counts) = engine_counts {
        let mut extra = Map::new();
        extra.insert("engines".into(), counts);
        provenance.extra = Some(extra);
    }

    // No aggregate session WAV exists on disk, so no `path`/`content_hash`;
    // duration/rate/bytes describe the transcribed chunks.
    let media = Media {
        duration_seconds: segments.last().map(|s| s.end),
        sample_rate: uniform_sample_rate,
        channels: Some(1),
        bytes: (total_bytes > 0).then_some(total_bytes),
        ..Media::default()
    };

    TranscriptV1::new(segments)
        .with_provenance(provenance)
        .with_media(media)
}

/// `Some(v)` when every item is `Some` of the same value.
fn uniform<T: PartialEq>(mut values: impl Iterator<Item = Option<T>>) -> Option<T> {
    let first = values.next()??;
    for v in values {
        if v.as_ref() != Some(&first) {
            return None;
        }
    }
    Some(first)
}

/// Most frequent engine label; when labels disagree, also return the full
/// count map for `provenance.extra` (contract §2.1).
fn majority_engine(chunks: &[ChunkTranscript]) -> (Option<String>, Option<Value>) {
    let mut counts: std::collections::BTreeMap<&str, u64> = Default::default();
    for c in chunks {
        if let Some(e) = &c.engine {
            *counts.entry(e).or_default() += 1;
        }
    }
    let majority = counts
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(e, _)| (*e).to_string());
    let extra = (counts.len() > 1).then(|| {
        Value::Object(
            counts
                .into_iter()
                .map(|(e, n)| (e.to_string(), Value::from(n)))
                .collect(),
        )
    });
    (majority, extra)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use lumen_types::{SourceEvent, SourceKind};
    use serde_json::json;

    fn ts(secs_offset: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 25, 9, 30, 0).unwrap() + chrono::Duration::seconds(secs_offset)
    }

    fn chunk(offset_secs: i64, ordinal: u64, audio_bytes: u64, text: &str) -> ChunkTranscript {
        ChunkTranscript {
            event_id: Uuid::new_v4(),
            ts: ts(offset_secs),
            ordinal,
            sample_rate: 16_000,
            audio_bytes: Some(audio_bytes),
            payload_duration_ms: None,
            text: text.into(),
            confidence: Some(0.9),
            language: Some("zh-CN".into()),
            engine: Some("sensevoice".into()),
        }
    }

    /// 3 seconds of 16 kHz mono s16le = 96 000 bytes.
    const THREE_SECONDS: u64 = 96_000;

    #[test]
    fn schema_constants_match_contract() {
        assert_eq!(DERIVED_TRANSCRIPT_V1, "transcript.v1");
        assert_eq!(lumen_transcript::SCHEMA_ID, "lumen-transcript.v1");
        let doc = build_document(vec![chunk(0, 1, THREE_SECONDS, "你好")]);
        let json = doc.to_json_string().unwrap();
        assert!(json.contains(r#""schema":"lumen-transcript.v1""#));
    }

    #[test]
    fn synthesizes_contiguous_times_from_event_ts() {
        let doc = build_document(vec![
            chunk(0, 1, THREE_SECONDS, "one"),
            chunk(3, 2, THREE_SECONDS, "two"),
        ]);
        let s = &doc.segments;
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].start, s[0].end), (0.0, 3.0));
        assert_eq!((s[1].start, s[1].end), (3.0, 6.0));
        assert_eq!(s[0].text, "one");
        assert_eq!(s[0].confidence, Some(0.9));
        assert_eq!(doc.media.as_ref().unwrap().duration_seconds, Some(6.0));
        assert_eq!(doc.media.as_ref().unwrap().sample_rate, Some(16_000));
        let p = doc.provenance.as_ref().unwrap();
        assert_eq!(p.app, "lumen-navi");
        assert_eq!(p.engine.as_deref(), Some("sensevoice"));
        assert_eq!(p.language.as_deref(), Some("zh-CN"));
        // Uniform language lives on provenance, not per segment.
        assert!(s.iter().all(|seg| seg.language.is_none()));
    }

    #[test]
    fn missing_chunk_leaves_hole_in_timeline() {
        // Second chunk (3s..6s) was silence-dropped; third starts at +10s.
        let doc = build_document(vec![
            chunk(0, 1, THREE_SECONDS, "one"),
            chunk(10, 3, THREE_SECONDS, "three"),
        ]);
        let s = &doc.segments;
        assert_eq!((s[0].start, s[0].end), (0.0, 3.0));
        assert_eq!((s[1].start, s[1].end), (10.0, 13.0)); // hole kept, not pulled to 3.0
    }

    #[test]
    fn overlapping_ts_clamps_start_without_overlap() {
        // Timestamp jitter: second chunk stamped only 1s after the first,
        // but the first carries 3s of audio.
        let doc = build_document(vec![
            chunk(0, 1, THREE_SECONDS, "one"),
            chunk(1, 2, THREE_SECONDS, "two"),
        ]);
        let s = &doc.segments;
        assert_eq!((s[0].start, s[0].end), (0.0, 3.0));
        assert_eq!((s[1].start, s[1].end), (3.0, 6.0)); // clamped, duration kept
        assert!(s[1].start >= s[0].end);
    }

    #[test]
    fn out_of_order_chunks_are_sorted_before_synthesis() {
        let doc = build_document(vec![
            chunk(6, 3, THREE_SECONDS, "three"),
            chunk(0, 1, THREE_SECONDS, "one"),
            chunk(3, 2, THREE_SECONDS, "two"),
        ]);
        let texts: Vec<&str> = doc.segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["one", "two", "three"]);
        assert_eq!(doc.segments[0].start, 0.0);
        assert!(doc.segments.windows(2).all(|w| w[0].end <= w[1].start));
    }

    #[test]
    fn mixed_language_and_engine_degrade_per_contract() {
        let mut a = chunk(0, 1, THREE_SECONDS, "one");
        a.language = Some("zh-CN".into());
        a.engine = Some("sensevoice".into());
        let mut b = chunk(3, 2, THREE_SECONDS, "two");
        b.language = Some("en".into());
        b.engine = Some("whisper".into());
        let mut c = chunk(6, 3, THREE_SECONDS, "three");
        c.language = Some("zh-CN".into());
        c.engine = Some("sensevoice".into());

        let doc = build_document(vec![a, b, c]);
        let p = doc.provenance.as_ref().unwrap();
        // Majority engine wins; disagreement recorded in extra.
        assert_eq!(p.engine.as_deref(), Some("sensevoice"));
        let engines = p.extra.as_ref().unwrap().get("engines").unwrap();
        assert_eq!(engines["sensevoice"], 2);
        assert_eq!(engines["whisper"], 1);
        // Mixed language → per-segment, no provenance language.
        assert!(p.language.is_none());
        assert_eq!(doc.segments[0].language.as_deref(), Some("zh-CN"));
        assert_eq!(doc.segments[1].language.as_deref(), Some("en"));
    }

    #[test]
    fn duration_falls_back_to_payload_ms_without_audio_bytes() {
        let mut c = chunk(0, 1, 0, "one");
        c.audio_bytes = None;
        c.payload_duration_ms = Some(2_500);
        let doc = build_document(vec![c]);
        assert_eq!(doc.segments[0].end, 2.5);
        // No byte counts known → media.bytes omitted.
        assert!(doc.media.as_ref().unwrap().bytes.is_none());
    }

    // ---- store-backed integration ----------------------------------------

    fn audio_event(session: Uuid, at: DateTime<Utc>, ordinal: u64) -> SourceEvent {
        let mut event = SourceEvent::new(
            SourceKind::Audio,
            event_kind::AUDIO_CHUNK_V1,
            json!({
                "payload_version": 1,
                "sample_rate": 16_000,
                "channels": 1,
                "duration_ms": 3_000,
                "session_ordinal": ordinal,
            }),
        )
        .with_session(session);
        event.ts = at;
        event
    }

    fn body(text: &str, audio_bytes: u64) -> String {
        json!({
            "payload_version": 1,
            "text": text,
            "confidence": 0.91,
            "language": "zh-CN",
            "engine": "sensevoice",
            "audio_bytes": audio_bytes,
        })
        .to_string()
    }

    #[tokio::test]
    async fn exports_session_from_store_sorted_by_ts() {
        use lumen_store::EventStore;

        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let session = Uuid::new_v4();
        let other_session = Uuid::new_v4();

        // Insert out of order; export must sort by event ts.
        for (offset, ordinal, text) in [(3i64, 2u64, "two"), (0, 1, "one")] {
            let event = audio_event(session, ts(offset), ordinal);
            let eid = event.id;
            store.append(vec![event]).await.unwrap();
            store
                .insert_derived(eid, DERIVED_TRANSCRIPT_V1, body(text, THREE_SECONDS))
                .unwrap();
        }
        // Chunk without a transcript (silence): must not appear.
        let silent = audio_event(session, ts(6), 3);
        store.append(vec![silent]).await.unwrap();
        // Other session must not leak in.
        let foreign = audio_event(other_session, ts(1), 1);
        let fid = foreign.id;
        store.append(vec![foreign]).await.unwrap();
        store
            .insert_derived(fid, DERIVED_TRANSCRIPT_V1, body("foreign", THREE_SECONDS))
            .unwrap();

        let doc = export_session_transcript(&store, session).unwrap();
        let texts: Vec<&str> = doc.segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["one", "two"]);
        assert_eq!(doc.segments[0].start, 0.0);
        assert_eq!(doc.segments[1].end, 6.0);
        assert_eq!(doc.media.as_ref().unwrap().bytes, Some(2 * THREE_SECONDS));
        // Segment ids are the chunk event ids (contract §2.1).
        assert!(doc.segments.iter().all(|s| s
            .id
            .as_deref()
            .is_some_and(|id| Uuid::parse_str(id).is_ok())));
    }

    #[tokio::test]
    async fn empty_session_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        let missing = Uuid::new_v4();
        let err = export_session_transcript(&store, missing).unwrap_err();
        assert!(matches!(err, TranscriptExportError::EmptySession(id) if id == missing));
    }
}
