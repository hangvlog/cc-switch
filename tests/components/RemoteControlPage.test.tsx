import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  listen: vi.fn(), unlisten: vi.fn(), start: vi.fn(), send: vi.fn(),
  stop: vi.fn(), plusPlus: vi.fn(), toastError: vi.fn(), toastSuccess: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@/lib/api/remote", () => ({
  remoteApi: {
    start: mocks.start, send: mocks.send, stop: mocks.stop,
    codexPlusPlusStatus: mocks.plusPlus,
  },
}));
vi.mock("sonner", () => ({
  toast: { error: mocks.toastError, success: mocks.toastSuccess },
}));

import { RemoteControlPage } from "@/components/remote/RemoteControlPage";

class FakeWebSocket {
  static readonly OPEN = 1;
  static instances: FakeWebSocket[] = [];
  readonly url: string;
  readyState = FakeWebSocket.OPEN;
  sent: string[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }
  send(payload: string) { this.sent.push(payload); }
  close() { this.closed = true; }
  open() { this.onopen?.(); }
  message(payload: unknown) { this.onmessage?.({ data: JSON.stringify(payload) }); }
}

function mockAccountRequests() {
  vi.stubGlobal("fetch", vi.fn(async (input: string) => {
    if (input.endsWith("/auth/login")) {
      return {
        ok: true, status: 200,
        json: async () => ({
          code: 200,
          data: { token: "account-token", user: { id: 7, username: "hang" } },
        }),
      };
    }
    return {
      ok: true, status: 200,
      json: async () => ({ ticket: "one-time-ticket" }),
    };
  }));
}

async function login() {
  fireEvent.change(screen.getByLabelText("账号"), { target: { value: "hang" } });
  fireEvent.change(screen.getByLabelText("密码"), { target: { value: "secret" } });
  fireEvent.click(screen.getByRole("button", { name: /登录并自动连接/ }));
  await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
}

describe("RemoteControlPage", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.stubEnv("VITE_CODEX_REMOTE_API_URL", "http://relay.test");
    vi.stubGlobal("WebSocket", FakeWebSocket);
    vi.stubGlobal("crypto", { randomUUID: () => "desktop-device-uuid" });
    mockAccountRequests();
    FakeWebSocket.instances = [];
    mocks.listen.mockResolvedValue(mocks.unlisten);
    mocks.start.mockResolvedValue({ running: true });
    mocks.stop.mockResolvedValue({ running: false });
    mocks.send.mockResolvedValue(undefined);
    mocks.plusPlus.mockResolvedValue({ installed: false, summary: "optional" });
  });

  it("logs in and automatically starts an account-scoped bridge", async () => {
    render(<RemoteControlPage />);
    await login();

    expect(mocks.start).toHaveBeenCalledWith();
    expect(fetch).toHaveBeenCalledWith(
      "http://relay.test/api/codex-remote/account/socket-ticket",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ Authorization: "Bearer account-token" }),
      }),
    );
    const socket = FakeWebSocket.instances[0];
    expect(socket.url).toBe(
      "ws://relay.test/api/codex-remote/account/ws?ticket=one-time-ticket",
    );
    act(() => socket.open());
    act(() => socket.message({ type: "relay.peer", role: "mobile", online: true }));
    expect(await screen.findByText("手机已连接")).toBeInTheDocument();

    act(() => socket.message({ type: "relay.data", payload: '{"id":1}' }));
    await waitFor(() => expect(mocks.send).toHaveBeenCalledWith('{"id":1}'));
    const nativeListener = mocks.listen.mock.calls[0][1] as (event: { payload: string }) => void;
    act(() => nativeListener({ payload: '{"method":"turn/completed"}' }));
    expect(JSON.parse(socket.sent[0])).toEqual({
      type: "relay.data", payload: '{"method":"turn/completed"}',
    });
  });

  it("restores the account and connects without another login", async () => {
    localStorage.setItem(
      "codex-remote-account-v1",
      JSON.stringify({ token: "account-token", user: { id: 7, username: "hang" } }),
    );
    render(<RemoteControlPage />);
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    expect(screen.getByText(/已登录 hang/)).toBeInTheDocument();
    expect(screen.queryByLabelText("密码")).not.toBeInTheDocument();
  });

  it("stops the native app-server and account relay together", async () => {
    render(<RemoteControlPage />);
    await login();
    const socket = FakeWebSocket.instances[0];
    fireEvent.click(screen.getByRole("button", { name: /停止/ }));
    await waitFor(() => expect(mocks.stop).toHaveBeenCalledOnce());
    expect(socket.closed).toBe(true);
    expect(mocks.unlisten).toHaveBeenCalledOnce();
    expect(screen.getByText("已停止")).toBeInTheDocument();
  });

  it("uses account login as the default instead of a pairing code", () => {
    render(<RemoteControlPage />);
    expect(screen.getByText("登录 ClawKit 账号")).toBeInTheDocument();
    expect(screen.queryByText("移动端配对码")).not.toBeInTheDocument();
  });
});
