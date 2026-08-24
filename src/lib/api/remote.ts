import { invoke } from "@tauri-apps/api/core";

export interface CodexServerStatus {
  running: boolean;
  model?: string;
  models?: string[];
  availableQuota?: number;
  usedQuota?: number;
}

export interface CodexPlusPlusStatus {
  installed: boolean;
  summary: string;
}

export const remoteApi = {
  status: () => invoke<CodexServerStatus>("get_codex_remote_status"),
  start: (codexHomeOverride?: string) =>
    invoke<CodexServerStatus>("start_codex_remote_server", {
      codexHomeOverride: codexHomeOverride || null,
    }),
  send: (payload: string) =>
    invoke<void>("send_codex_remote_message", { payload }),
  stop: () => invoke<CodexServerStatus>("stop_codex_remote_server"),
  codexPlusPlusStatus: () =>
    invoke<CodexPlusPlusStatus>("get_codex_plus_plus_status"),
  launchCodexPlusPlus: () => invoke<void>("launch_codex_plus_plus"),
};
