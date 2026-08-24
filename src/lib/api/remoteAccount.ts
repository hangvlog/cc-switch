import { invoke } from "@tauri-apps/api/core";

export interface RemoteAccountUser {
  id?: number;
  username: string;
  nickname?: string | null;
  avatar?: string | null;
}

export interface RemoteAccountStatus {
  status: string;
  authenticated: boolean;
  user?: RemoteAccountUser | null;
  deviceId?: string;
  expiresAt?: number;
}

export interface RemoteSocketTicket {
  status: string;
  websocketUrl: string;
  expiresAt?: number;
  deviceId: string;
}

export const remoteAccountApi = {
  status: () => invoke<RemoteAccountStatus>("get_clawkit_account_status"),
  login: (username: string, password: string) =>
    invoke<RemoteAccountStatus>("login_clawkit_account", { username, password }),
  logout: () => invoke<RemoteAccountStatus>("logout_clawkit_account"),
  createSocketTicket: () =>
    invoke<RemoteSocketTicket>("create_clawkit_socket_ticket"),
};
