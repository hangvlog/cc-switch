import { invoke } from "@tauri-apps/api/core";

export interface CodexServerStatus {
  running: boolean;
  model?: string;
  models?: string[];
  availableQuota?: number;
  usedQuota?: number;
}

export interface ClawkitCodexConfigurationStatus {
  configured: boolean;
  model?: string;
  models?: string[];
  configPath: string;
  availableQuota?: number;
  usedQuota?: number;
  canRollback: boolean;
}

export interface ClawkitCodexModelOptions {
  defaultModel: string;
  models: string[];
}

export interface CodexPlusPlusStatus {
  installed: boolean;
  summary: string;
}

export interface DiagnosticBundleUpload {
  bundleId: string;
  url: string;
  expiresAt: number;
  expiresInSeconds: number;
}

export const remoteApi = {
  configurationStatus: () =>
    invoke<ClawkitCodexConfigurationStatus>(
      "get_clawkit_codex_configuration_status",
    ),
  modelOptions: () =>
    invoke<ClawkitCodexModelOptions>("get_clawkit_codex_model_options"),
  configure: (selectedModel?: string) =>
    invoke<ClawkitCodexConfigurationStatus>("configure_clawkit_codex", {
      selectedModel: selectedModel || null,
    }),
  uploadDiagnosticBundle: () =>
    invoke<DiagnosticBundleUpload>("upload_clawkit_diagnostic_bundle"),
  rollbackConfiguration: () =>
    invoke<ClawkitCodexConfigurationStatus>(
      "rollback_clawkit_codex_configuration",
    ),
  remoteStatus: () => invoke<CodexServerStatus>("get_codex_remote_status"),
  startRemote: (codexHomeOverride?: string) =>
    invoke<CodexServerStatus>("start_codex_remote_server", {
      codexHomeOverride: codexHomeOverride || null,
    }),
  send: (payload: string) =>
    invoke<void>("send_codex_remote_message", { payload }),
  stopRemote: () => invoke<CodexServerStatus>("stop_codex_remote_server"),
  codexPlusPlusStatus: () =>
    invoke<CodexPlusPlusStatus>("get_codex_plus_plus_status"),
  launchCodexPlusPlus: () => invoke<void>("launch_codex_plus_plus"),
};
