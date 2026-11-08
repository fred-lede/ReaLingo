/**
 * Whether the input is too quiet to be heard.
 *
 * A microphone at a low gain delivers audio a hundred times below what a remote
 * service expects: the transcript comes out empty, or the recognition is poor, and
 * nothing on screen says why. The local mode has no such problem — Whisper hears a
 * quiet microphone — so the warning belongs to the remote one only.
 */

/** Below this the input is quieter than speech should be. */
export const QUIET_INPUT_RMS = 0.02;

export function isInputTooQuiet(
  rms: number,
  context: { running: boolean; remote: boolean }
): boolean {
  if (!context.running || !context.remote) return false;
  return rms < QUIET_INPUT_RMS;
}
