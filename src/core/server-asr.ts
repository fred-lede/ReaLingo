/**
 * The ASR model the server reported, when it is worth showing.
 *
 * One ASR model is loaded on the server and the client cannot choose it, so the server
 * reports it and the client shows it. Only for a custom endpoint: the built-in protocols
 * pair with the vendor's own transcription model, which the server does not know — naming
 * the server's model there would be naming a different model.
 */
export function visibleServerAsr(url: string, reported: string): string {
  if (!url.trim() || !reported.trim()) return "";
  return reported.trim();
}
