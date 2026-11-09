/**
 * Whether the input is too quiet to be heard.
 *
 * The bar is not "quiet" but "no signal": a microphone at a low gain delivers audio a
 * hundred times below what a loud one does, and the server judges speech by the input
 * level rather than a fixed cutoff, so such audio is still transcribed. What must be
 * warned about is an input with nothing in it — a muted microphone, a wrong device —
 * where the transcript is empty and nothing on screen says why.
 *
 * The local mode has no such problem — Whisper hears a quiet microphone — so the warning
 * belongs to the remote one only.
 */

/** Below this the input carries no signal worth sending. */
export const QUIET_INPUT_RMS = 0.004;

export function isInputTooQuiet(
  rms: number,
  context: { running: boolean; remote: boolean }
): boolean {
  if (!context.running || !context.remote) return false;
  return rms < QUIET_INPUT_RMS;
}
