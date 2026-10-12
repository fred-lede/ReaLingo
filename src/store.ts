import { reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { load, type Store } from "@tauri-apps/plugin-store";
import { MODEL_QWEN3_5 } from "./languages";

export type Region = "beijing" | "singapore";
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

/** The wire protocol. Model and URL are free-form on top of it. */
export type Provider = "qwen" | "openai";

/** Where each provider's key lives in `Settings`; Qwen's predates there being two. */
const KEY_FIELD = { qwen: "apiKey", openai: "openaiKey" } as const;

export interface Settings {
  provider: Provider;
  apiKey: string;
  openaiKey: string;
  /** Empty: the provider's default endpoint. */
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
  provider: "qwen",
  apiKey: "",
  openaiKey: "",
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
  }

  keyringOk.value = (await invoke<{ keyring: boolean }>("platform")).keyring;
  if (keyringOk.value) {
    // Anything still in the file is from a build that predates the credential store (or a
    // hand-edited config). Move it across and blank it out — leaving the plaintext copy
    // behind would make the whole exercise pointless.
    for (const [provider, field] of Object.entries(KEY_FIELD)) {
      if (settings[field]) await invoke("set_api_key", { provider, key: settings[field] });
      else settings[field] = await invoke<string>("get_api_key", { provider });
    }
  }

  applyTheme();
  if (persist) {
    watch(settings, (v) => void store?.set(KEY, persistable(v)), { deep: true });
    if (keyringOk.value) {
      for (const [provider, field] of Object.entries(KEY_FIELD)) {
        watch(() => settings[field], (key) => saveApiKey(provider, key));
      }
    }
  }
  watch(() => settings.theme, applyTheme);
}

/** The key of the provider in use — the one a session will actually send. */
export function activeKey(): string {
  return settings[KEY_FIELD[settings.provider]];
}

export function setActiveKey(key: string) {
  settings[KEY_FIELD[settings.provider]] = key;
}

/** What goes to settings.json — never an API key once the credential store holds it. */
function persistable(v: Settings) {
  const plain = JSON.parse(JSON.stringify(v)) as Settings;
  if (keyringOk.value) for (const field of Object.values(KEY_FIELD)) plain[field] = "";
  return plain;
}

// Debounced: one credential-store write per pause, not one per keystroke.
const keyTimers: Record<string, number | undefined> = {};
function saveApiKey(provider: string, key: string) {
  clearTimeout(keyTimers[provider]);
  keyTimers[provider] = window.setTimeout(() => void invoke("set_api_key", { provider, key }), 400);
}

const dark = window.matchMedia("(prefers-color-scheme: dark)");
dark.addEventListener("change", applyTheme);

/** The theme actually in effect, with "system" already resolved. */
export const resolvedTheme = ref<"light" | "dark">("light");

export function applyTheme() {
  resolvedTheme.value = settings.theme === "system" ? (dark.matches ? "dark" : "light") : settings.theme;
  document.documentElement.dataset.theme = resolvedTheme.value;
}
