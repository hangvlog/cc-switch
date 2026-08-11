export interface RemoteAccountUser {
  id: number;
  username: string;
  nickname?: string | null;
  avatar?: string | null;
}

export interface RemoteAccountSession {
  token: string;
  user: RemoteAccountUser;
}

const SESSION_KEY = "codex-remote-account-v1";
const DEVICE_KEY = "codex-remote-device-id-v1";

function responseError(body: any, fallback: string) {
  return body?.message || body?.detail || body?.data?.error || fallback;
}

export function loadRemoteAccount(): RemoteAccountSession | null {
  try {
    const value = localStorage.getItem(SESSION_KEY);
    return value ? (JSON.parse(value) as RemoteAccountSession) : null;
  } catch {
    return null;
  }
}

export function saveRemoteAccount(session: RemoteAccountSession) {
  localStorage.setItem(SESSION_KEY, JSON.stringify(session));
}

export function clearRemoteAccount() {
  localStorage.removeItem(SESSION_KEY);
}

export function getDesktopDeviceId() {
  const existing = localStorage.getItem(DEVICE_KEY);
  if (existing) return existing;
  const id = `desktop-${crypto.randomUUID()}`;
  localStorage.setItem(DEVICE_KEY, id);
  return id;
}

export async function loginRemoteAccount(
  apiBase: string,
  username: string,
  password: string,
): Promise<RemoteAccountSession> {
  const response = await fetch(`${apiBase}/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      username,
      password,
      device_id: getDesktopDeviceId(),
      product: "codex-remote",
    }),
  });
  const body = await response.json().catch(() => null);
  if (!response.ok || body?.code !== 200 || !body?.data?.token) {
    throw new Error(responseError(body, `登录失败 (${response.status})`));
  }
  return { token: body.data.token, user: body.data.user };
}

export async function createRemoteSocketTicket(
  apiBase: string,
  token: string,
) {
  const response = await fetch(
    `${apiBase}/api/codex-remote/account/socket-ticket`,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({
        role: "desktop",
        device_id: getDesktopDeviceId(),
        device_name: "ClawKit Desktop",
      }),
    },
  );
  const body = await response.json().catch(() => null);
  if (!response.ok || !body?.ticket) {
    const error = new Error(
      responseError(body, `创建安全连接失败 (${response.status})`),
    );
    (error as Error & { status?: number }).status = response.status;
    throw error;
  }
  return body.ticket as string;
}

export function accountWebsocketUrl(apiBase: string, ticket: string) {
  const base = apiBase.replace(/^http:/, "ws:").replace(/^https:/, "wss:");
  return `${base}/api/codex-remote/account/ws?${new URLSearchParams({ ticket })}`;
}
