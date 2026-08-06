import type {
  BrowserArtifact,
  BrowserObservation,
  LocalStoreStats,
  QueueState,
} from "./types";

const DB_NAME = "lumen-navi-browser";
const DB_VERSION = 1;
const OBSERVATIONS = "observations";
const ARTIFACTS = "artifacts";

export type SyncStatus = "pending" | "synced" | "rejected";

export interface StoredObservation extends BrowserObservation {
  local_stored_at: string;
  sync_status: SyncStatus;
  sync_updated_at?: string;
  sync_reason?: string;
}

export interface StoredArtifact extends BrowserArtifact {
  local_stored_at: string;
}

let database: Promise<IDBDatabase> | undefined;

function openDatabase(): Promise<IDBDatabase> {
  if (database) return database;
  database = new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(OBSERVATIONS)) {
        const store = db.createObjectStore(OBSERVATIONS, { keyPath: "id" });
        store.createIndex("sync_status", "sync_status", { unique: false });
        store.createIndex("ts", "ts", { unique: false });
      }
      if (!db.objectStoreNames.contains(ARTIFACTS)) {
        db.createObjectStore(ARTIFACTS, { keyPath: "event_id" });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("open local browser store"));
    request.onblocked = () => reject(new Error("local browser store upgrade blocked"));
  });
  return database;
}

function transactionDone(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error ?? new Error("local store transaction"));
    transaction.onabort = () => reject(transaction.error ?? new Error("local store transaction aborted"));
  });
}

function requestResult<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("local store request"));
  });
}

export async function storeLocalObservation(
  observation: BrowserObservation,
  artifact?: BrowserArtifact,
): Promise<void> {
  const db = await openDatabase();
  const transaction = db.transaction(
    artifact ? [OBSERVATIONS, ARTIFACTS] : [OBSERVATIONS],
    "readwrite",
  );
  const done = transactionDone(transaction);
  const observations = transaction.objectStore(OBSERVATIONS);
  const existing = await requestResult(observations.get(observation.id)) as
    | StoredObservation
    | undefined;
  const now = new Date().toISOString();
  observations.put({
    ...observation,
    local_stored_at: existing?.local_stored_at ?? now,
    sync_status: existing?.sync_status ?? "pending",
    sync_updated_at: existing?.sync_updated_at,
    sync_reason: existing?.sync_reason,
  } satisfies StoredObservation);
  if (artifact) {
    transaction.objectStore(ARTIFACTS).put({
      ...artifact,
      local_stored_at: now,
    } satisfies StoredArtifact);
  }
  await done;
}

export async function backfillQueue(queue: QueueState): Promise<void> {
  const artifacts = new Map(queue.artifacts.map((artifact) => [artifact.event_id, artifact]));
  const observations = queue.pendingGap
    ? [queue.pendingGap, ...queue.observations]
    : queue.observations;
  for (const observation of observations) {
    await storeLocalObservation(observation, artifacts.get(observation.id));
  }
}

export async function markLocalSync(
  eventIds: string[],
  status: Exclude<SyncStatus, "pending">,
  reason?: string,
): Promise<void> {
  if (eventIds.length === 0) return;
  const db = await openDatabase();
  const transaction = db.transaction(OBSERVATIONS, "readwrite");
  const done = transactionDone(transaction);
  const store = transaction.objectStore(OBSERVATIONS);
  const now = new Date().toISOString();
  for (const eventId of eventIds) {
    const stored = await requestResult(store.get(eventId)) as StoredObservation | undefined;
    if (!stored) continue;
    store.put({
      ...stored,
      sync_status: status,
      sync_updated_at: now,
      sync_reason: reason,
    } satisfies StoredObservation);
  }
  await done;
}

export async function localStoreStats(): Promise<LocalStoreStats> {
  const db = await openDatabase();
  const transaction = db.transaction([OBSERVATIONS, ARTIFACTS], "readonly");
  const done = transactionDone(transaction);
  const observations = transaction.objectStore(OBSERVATIONS);
  const statusIndex = observations.index("sync_status");
  const [observationCount, artifactCount, pendingSync, synced, rejected] = await Promise.all([
    requestResult(observations.count()),
    requestResult(transaction.objectStore(ARTIFACTS).count()),
    requestResult(statusIndex.count("pending")),
    requestResult(statusIndex.count("synced")),
    requestResult(statusIndex.count("rejected")),
  ]);
  await done;
  return {
    observations: observationCount,
    artifacts: artifactCount,
    pendingSync,
    synced,
    rejected,
  };
}

export async function readLocalArchive(limit = 5_000): Promise<{
  observations: StoredObservation[];
  artifacts: StoredArtifact[];
}> {
  const db = await openDatabase();
  const transaction = db.transaction(OBSERVATIONS, "readonly");
  const done = transactionDone(transaction);
  const observations = await readLatestObservations(
    transaction.objectStore(OBSERVATIONS).index("ts"),
    Math.max(1, limit),
  );
  await done;
  const artifactIds = observations
    .filter((item) => item.kind === "browser.document_ready.v1")
    .map((item) => item.id);
  const artifactTransaction = db.transaction(ARTIFACTS, "readonly");
  const artifactsDone = transactionDone(artifactTransaction);
  const artifactStore = artifactTransaction.objectStore(ARTIFACTS);
  const artifacts = (await Promise.all(
    artifactIds.map((eventId) => requestResult(artifactStore.get(eventId))),
  ) as Array<StoredArtifact | undefined>).filter(
    (item): item is StoredArtifact => item !== undefined,
  );
  await artifactsDone;
  return { observations, artifacts };
}

function readLatestObservations(index: IDBIndex, limit: number): Promise<StoredObservation[]> {
  return new Promise((resolve, reject) => {
    const observations: StoredObservation[] = [];
    const request = index.openCursor(null, "prev");
    request.onsuccess = () => {
      const cursor = request.result;
      if (!cursor || observations.length >= limit) {
        resolve(observations);
        return;
      }
      observations.push(cursor.value as StoredObservation);
      cursor.continue();
    };
    request.onerror = () => reject(request.error ?? new Error("read local browser archive"));
  });
}
