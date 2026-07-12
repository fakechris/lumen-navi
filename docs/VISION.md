# Lumen Navi — Vision

> Recorded: 2026-07-11 · Updated with media-first skeleton  
> Repo: https://github.com/fakechris/lumen-navi

## Why

Useful digital context is created continuously and lost immediately. Lumen Navi **captures the stream locally**, under user control, and turns it into durable, queryable context for people and agents.

Greenfield **Rust** product — architecture-first, not constrained by prior prototypes.

## Core idea

```
 continuous multi-source intake (media first)
            │
            ▼
     normalize + store (raw)
            │
            ▼
   process / enrich / index
            │
            ▼
   memory, timeline, agents, UI
```

**Intake is the foundation.** Everything else is a consumer of a reliable event stream.

## Sources

| Source | Priority | Notes |
|--------|----------|-------|
| Screen | **First** | Continuous / event-driven screenshots |
| Audio | **First** | Mic; system audio later |
| Video | Optional early | Higher cost; same pipeline |
| Browser (Chrome) | **Later** | Navigation & page signals via extension |
| Coding agents | Later | Conversation / tool transcripts |
| Lumen ASR sessions | Optional later | Separate product: https://github.com/fakechris/lumen-asr |
| Future | Open | Pluggable adapters |

## Three planes

1. **Observe** — capture (Navi-owned media path)  
2. **Memory** — store + process  
3. **Act** (optional) — computer-use via open-source **[cua-driver](https://github.com/trycua/cua)** (MIT only)

## Principles

1. Local-first raw media  
2. Consent-shaped sources  
3. Source-agnostic core envelope  
4. Cheap signals before heavy media  
5. Process later — capture never blocked by enrich  
6. Privacy as product (deny-list, pause, wipe, redaction)  
7. Rust core; edge adapters may be JS/TS  
8. Fail soft on enrichment  

## Related products

- **[Lumen ASR](https://github.com/fakechris/lumen-asr)** — dictation; separate repo; may later feed Navi or share engine patterns.  
- **[cua-driver](https://github.com/trycua/cua)** — MIT computer-use driver for optional Act plane only (not observe).  

## Success (directional)

- 2+ media sources → coherent local timeline of “what I was doing”  
- New source is a weekend-sized adapter against stable APIs  
- Heavy process failures never drop raw capture  
- User can pause, wipe, and export  
