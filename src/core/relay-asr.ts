/**
 * A relay's own ASR switch, for a router that loads one ASR model and serves
 * `GET/POST /api/asr/config` (AetherMesh's `runtime/asr/asr_manager.py`). Such a relay can
 * transcribe with a model on its own GPU, on its CPU, or on another machine's ASR worker
 * (`node:<id>`), and the dashboard offers that choice — so the app can offer it too, and a
 * user never has to leave it for the dashboard.
 *
 * Only a custom endpoint has this route: the built-in vendors answer nothing here, so every
 * failure here is an empty result and the row stays hidden rather than a complaint.
 */
export function relayAsrBase(url: string): string {
  const origin = /^(wss?|https?):\/\/[^/?#\s]+/i.exec(url.trim());
  if (!origin) return "";
  const scheme = origin[1].toLowerCase();
  const http = scheme === "wss" || scheme === "https" ? "https" : "http";
  return `${http}${origin[0].slice(scheme.length)}`;
}

/** One target the relay can aim at; a disabled one is shown, but cannot be chosen. */
export interface RelayAsrTarget {
  id: string;
  label: string;
  /** The relay cannot aim at it right now — its ASR worker is not reachable. */
  disabled?: boolean;
}

/** The relay's own answer, shaped for the picker: one option per target it can aim at. */
export interface RelayAsrSnapshot {
  /** The relay answered; it picks the ASR model itself, so the client cannot. */
  ok: boolean;
  active: string;
  model: string;
  state: string;
  targets: RelayAsrTarget[];
  /** The relay could not read the cluster's node list, so the nodes are missing. */
  nodesUnknown: boolean;
}

export async function relayAsrConfig(url: string, key: string): Promise<RelayAsrSnapshot> {
  const empty: RelayAsrSnapshot = { ok: false, active: "", model: "", state: "", targets: [], nodesUnknown: false };
  const base = relayAsrBase(url);
  if (!base) return empty;
  try {
    const r = await fetch(`${base}/api/asr/config`, { headers: { Authorization: `Bearer ${key}` } });
    if (!r.ok) return empty;
    const v = await r.json();
    const targets = v?.targets;
    const local = (targets?.local ?? []) as { id?: string; name?: string }[];
    const nodes = (targets?.nodes ?? []) as { id?: string; reachable?: boolean; engine?: string; model?: string }[];
    // A node whose worker is down stays in the list, greyed out: the manager would reject it
    // with "node '<id>' has no reachable asr_worker", and a silent gap would read as if the
    // machine were not part of the cluster at all.
    const options = [
      ...local.map((t) => ({ id: String(t.id ?? ""), label: String(t.name || t.id || "") })),
      ...nodes.map((n) => {
        const named = [String(n.engine ?? ""), String(n.model ?? "")].filter((s) => s).join(" ");
        const label = [String(n.id ?? "").replace(/^node:/, ""), named].filter((s) => s).join(" · ");
        return { id: String(n.id ?? ""), label, disabled: n.reachable === false };
      }),
    ].filter((o) => o.id);
    const active = v?.active ?? {};
    return {
      ok: true,
      active: String(active?.target_id ?? ""),
      model: String(active?.model ?? ""),
      state: String(v?.state ?? ""),
      targets: options,
      nodesUnknown: targets?.nodes_unreachable === true,
    };
  } catch {
    return empty;
  }
}

/** Ask the relay to load another target. The switch is relay-wide: every client of it moves. */
export async function relayAsrSelect(url: string, key: string, targetId: string): Promise<boolean> {
  const base = relayAsrBase(url);
  if (!base || !targetId) return false;
  try {
    const r = await fetch(
      `${base}/api/asr/config`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
        body: JSON.stringify({ target_id: targetId }),
      }
    );
    return r.ok;
  } catch {
    return false;
  }
}
