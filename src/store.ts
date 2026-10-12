import { reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { load, type Store } from "@tauri-apps/plugin-store";
import { MODEL_QWEN3_5, MODEL_QWEN3_8 } from "./languages";

/** Only the regions that serve the LiveTranslate models — see `config::Region` in Rust. */
export type Region = "beijing" | "singapore";

/** Model Studio's region id: part of every console link. */
export const REGION_ID: Record<Region, string> = { beijing: "cn-beijing", singapore: "ap-southeast-1" };
export type Theme = "system" | "light" | "dark";
export type SubMode = "both" | "target" | "source";
export type SubAlign = "left" | "center" | "right";

/** Where the overlay was last parked, in physical pixels. Null until it is first moved. */
export interface SubRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface SubtitleStyle {
  show: boolean;
  mode: SubMode;
  opacity: number; // background alpha, 0..1
  fontSize: number;
  color: string; // translation
  srcColor: string; // source transcript
  outline: boolean;
  align: SubAlign;
  /** Not style, but it travels with it: the overlay reports it, the main window stores it. */
  rect: SubRect | null;
  /** true: wrap and grow the window to the text. false: one line per row, tail-aligned. */
  grow: boolean;
  locked: boolean; // click-through
}

/**
 * The message format, which follows the protocol rather than the vendor: Qwen3.8 speaks a
 * different schema from Qwen3/3.5. Model and URL are free-form on top of it.
 */
export type Protocol = "qwen35" | "qwen38" | "openai" | "gemini";

/** Where each protocol's key lives. A key belongs to the account, so both Qwens share one. */
const KEY_FIELD = { qwen35: "apiKey", qwen38: "apiKey", openai: "openaiKey", gemini: "geminiKey" } as const;
/** One protocol per credential-store slot, for the loops that visit every key once. */
const KEY_SLOTS = ["qwen35", "openai", "gemini"] as const;

export interface Settings {
  protocol: Protocol;
  apiKey: string;
  openaiKey: string;
  geminiKey: string;
  /** Empty: the protocol's default endpoint. */
  url: string;
  workspaceId: string;
  region: Region;
  sourceLang: string;
  targetLang: string;
  model: string;
  /** Off by default: translations are private, so nothing hits the disk unless asked. */
  history: boolean;
  /** Hotwords: source term -> preferred translation. Sent with `session.update`. */
  hotwords: Record<string, string>;
  /** Read the translation aloud. Only takes effect where `canSpeak` allows it. */
  speak: boolean;
  voice: string;
  theme: Theme;
  sub: SubtitleStyle;
}

export const settings = reactive<Settings>({
  protocol: "qwen35",
  apiKey: "",
  openaiKey: "",
  geminiKey: "",
  url: "",
  workspaceId: "",
  region: "beijing",
  sourceLang: "auto",
  targetLang: "en",
  model: MODEL_QWEN3_5,
  history: false,
  hotwords: {},
  speak: false,
  voice: "Tina",
  theme: "system",
  sub: {
    show: false,
    mode: "both",
    opacity: 0.5,
    fontSize: 30,
    color: "#ffffff",
    srcColor: "#a8c0dd",
    outline: true,
    align: "center",
    rect: null,
    grow: true,
    // Default to click-through: an always-on-top overlay otherwise eats every click inside
    // its window rect, transparent parts included. Turn it off to reposition the bar.
    locked: true,
  },
});

const FILE = "settings.json";
const KEY = "settings";
let store: Store | null = null;

/**
 * Whether the API key lives in the OS credential store. False only where there is none —
 * Linux without a Secret Service provider — and then it stays in settings.json as before.
 */
export const keyringOk = ref(true);

/** Load persisted settings; afterwards every change is written back automatically. */
export async function initSettings(persist = true) {
  store = await load(FILE, { autoSave: 200 });
  const saved = await store.get<Partial<Settings>>(KEY);
  if (saved) {
    Object.assign(settings, saved, { sub: { ...settings.sub, ...(saved.sub ?? {}) } });
    // Saved before the protocol was a setting: the model was what picked the 3.8 schema.
    if (!saved.protocol) settings.protocol = saved.model === MODEL_QWEN3_8 ? "qwen38" : "qwen35";
  }

  keyringOk.value = (await invoke<{ keyring: boolean }>("platform")).keyring;
  if (keyringOk.value) {
    // Anything still in the file is from a build that predates the credential store (or a
    // hand-edited config). Move it across and blank it out — leaving the plaintext copy
    // behind would make the whole exercise pointless.
    for (const protocol of KEY_SLOTS) {
      const field = KEY_FIELD[protocol];
      if (settings[field]) await invoke("set_api_key", { protocol, key: settings[field] });
      else settings[field] = await invoke<string>("get_api_key", { protocol });
    }
  }

  applyTheme();
  if (persist) {
    watch(settings, (v) => void store?.set(KEY, persistable(v)), { deep: true });
    if (keyringOk.value) {
      for (const protocol of KEY_SLOTS) {
        watch(() => settings[KEY_FIELD[protocol]], (key) => saveApiKey(protocol, key));
      }
    }
  }
  watch(() => settings.theme, applyTheme);
}

/** The key of the protocol in use — the one a session will actually send. */
export function activeKey(): string {
  return settings[KEY_FIELD[settings.protocol]];
}

export function setActiveKey(key: string) {
  settings[KEY_FIELD[settings.protocol]] = key;
}

/** What goes to settings.json — never an API key once the credential store holds it. */
function persistable(v: Settings) {
  const plain = JSON.parse(JSON.stringify(v)) as Settings;
  if (keyringOk.value) for (const field of Object.values(KEY_FIELD)) plain[field] = "";
  return plain;
}

// Debounced: one credential-store write per pause, not one per keystroke.
const keyTimers: Partial<Record<Protocol, number>> = {};
function saveApiKey(protocol: Protocol, key: string) {
  clearTimeout(keyTimers[protocol]);
  keyTimers[protocol] = window.setTimeout(() => void invoke("set_api_key", { protocol, key }), 400);
}

const dark = window.matchMedia("(prefers-color-scheme: dark)");
dark.addEventListener("change", applyTheme);

/** The theme actually in effect, with "system" already resolved. */
export const resolvedTheme = ref<"light" | "dark">("light");

export function applyTheme() {
  resolvedTheme.value = settings.theme === "system" ? (dark.matches ? "dark" : "light") : settings.theme;
  document.documentElement.dataset.theme = resolvedTheme.value;
}
