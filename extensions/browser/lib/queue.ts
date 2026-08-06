import type {
  BrowserArtifact,
  BrowserBatch,
  BrowserObservation,
  QueueState,
  Settings,
} from "./types";

const GAP_VISIT_ID = "00000000-0000-4000-8000-000000000000";

export function appendObservation(
  current: QueueState,
  observation: BrowserObservation,
  artifact: BrowserArtifact | undefined,
  maxQueueSize: number,
  maxQueueBytes: number,
): QueueState {
  const observations = [...current.observations];
  const artifacts = [...current.artifacts];
  let droppedEvents = current.droppedEvents;
  let pendingGap = current.pendingGap;
  const maximum = Math.max(1, maxQueueSize);

  observations.push(observation);
  if (artifact) artifacts.push(artifact);

  while (
    observations.length > maximum ||
    artifactBytes(artifacts) > Math.max(0, maxQueueBytes)
  ) {
    const dropped = observations.shift();
    if (dropped) {
      droppedEvents += 1;
      const remaining = artifacts.filter((item) => item.event_id !== dropped.id);
      artifacts.splice(0, artifacts.length, ...remaining);
    }
  }
  if (droppedEvents > current.droppedEvents) {
    pendingGap = pendingGap ?? {
      id: crypto.randomUUID(),
      kind: "browser.gap.v1",
      ts: new Date().toISOString(),
      visit_id: GAP_VISIT_ID,
      payload: {},
    };
    pendingGap = {
      ...pendingGap,
      payload: { dropped_events: droppedEvents },
    };
  }
  return { observations, artifacts, pendingGap, droppedEvents };
}

function artifactBytes(artifacts: BrowserArtifact[]): number {
  const encoder = new TextEncoder();
  return artifacts.reduce((total, artifact) => total + encoder.encode(artifact.body).byteLength, 0);
}

export async function buildBatch(
  state: QueueState,
  settings: Settings,
  configHash: string,
): Promise<BrowserBatch> {
  const batchSize = Math.max(1, settings.batchSize);
  const observations: BrowserObservation[] = [];
  if (state.pendingGap) observations.push(state.pendingGap);
  observations.push(...state.observations.slice(0, batchSize - observations.length));
  const ids = new Set(observations.map((item) => item.id));

  return {
    installation_id: settings.installationId,
    schema_version: 1,
    capture_profile_version: settings.captureProfileVersion,
    config_hash: configHash,
    observations,
    artifacts: state.artifacts.filter((item) => ids.has(item.event_id)),
  };
}

export function acknowledge(state: QueueState, eventIds: string[]): QueueState {
  const acknowledged = new Set(eventIds);
  return {
    observations: state.observations.filter((item) => !acknowledged.has(item.id)),
    artifacts: state.artifacts.filter((item) => !acknowledged.has(item.event_id)),
    pendingGap:
      state.pendingGap && acknowledged.has(state.pendingGap.id) ? undefined : state.pendingGap,
    droppedEvents: state.droppedEvents,
  };
}

export function discardObservations(
  state: QueueState,
  eventIds: string[],
  reason: string,
): QueueState {
  const rejected = new Set(eventIds);
  const droppedNow = state.observations.filter((item) => rejected.has(item.id)).length;
  if (droppedNow === 0) return state;
  const droppedEvents = state.droppedEvents + droppedNow;
  const pendingGap = state.pendingGap ?? {
    id: crypto.randomUUID(),
    kind: "browser.gap.v1" as const,
    ts: new Date().toISOString(),
    visit_id: GAP_VISIT_ID,
    payload: {},
  };
  return {
    observations: state.observations.filter((item) => !rejected.has(item.id)),
    artifacts: state.artifacts.filter((item) => !rejected.has(item.event_id)),
    pendingGap: {
      ...pendingGap,
      payload: { dropped_events: droppedEvents, reason },
    },
    droppedEvents,
  };
}
