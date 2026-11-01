export interface LocalDiagnostics {
  audioQueueMs: number | null;
  asrElapsedMs: number | null;
  translationQueueMs: number | null;
  translationElapsedMs: number | null;
  droppedChunks: number;
}

export function emptyLocalDiagnostics(): LocalDiagnostics {
  return {
    audioQueueMs: null,
    asrElapsedMs: null,
    translationQueueMs: null,
    translationElapsedMs: null,
    droppedChunks: 0,
  };
}

export function updateLocalDiagnostics(stats: LocalDiagnostics, payload: unknown): LocalDiagnostics {
  if (!payload || typeof payload !== "object") return stats;
  const event = payload as Record<string, unknown>;
  const valid = (value: unknown): value is number =>
    typeof value === "number" && Number.isFinite(value) && value >= 0;
  if (event.type === "local.asr" && valid(event.audioQueueChunks) && valid(event.elapsedMs)) {
    return { ...stats, audioQueueMs: event.audioQueueChunks * 100, asrElapsedMs: event.elapsedMs };
  }
  if (event.type === "local.translation" && valid(event.queueWaitMs) && valid(event.elapsedMs)) {
    return { ...stats, translationQueueMs: event.queueWaitMs, translationElapsedMs: event.elapsedMs };
  }
  if (event.type === "audio.capture_dropped" && valid(event.totalChunks)) {
    return { ...stats, droppedChunks: event.totalChunks };
  }
  return stats;
}
