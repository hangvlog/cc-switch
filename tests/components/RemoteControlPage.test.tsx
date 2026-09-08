import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  unlisten: vi.fn(),
  configurationStatus: vi.fn(),
  modelOptions: vi.fn(),
  configure: vi.fn(),
  uploadDiagnosticBundle: vi.fn(),
  rollbackConfiguration: vi.fn(),
  remoteStatus: vi.fn(),
  startRemote: vi.fn(),
  send: vi.fn(),
  stopRemote: vi.fn(),
  plusPlus: vi.fn(),
  launchPlusPlus: vi.fn(),
  accountStatus: vi.fn(),
  accountLogin: vi.fn(),
  accountLogout: vi.fn(),
  socketTicket: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
  copyText: vi.fn(),
  openExternal: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@/lib/api/remote", () => ({
  remoteApi: {
    configurationStatus: mocks.configurationStatus,
    modelOptions: mocks.modelOptions,
    configure: mocks.configure,
    uploadDiagnosticBundle: mocks.uploadDiagnosticBundle,
    rollbackConfiguration: mocks.rollbackConfiguration,
    remoteStatus: mocks.remoteStatus,
    startRemote: mocks.startRemote,
    send: mocks.send,
    stopRemote: mocks.stopRemote,
    codexPlusPlusStatus: mocks.plusPlus,
    launchCodexPlusPlus: mocks.launchPlusPlus,
  },
}));
vi.mock("@/lib/clipboard", () => ({ copyText: mocks.copyText }));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: mocks.openExternal },
}));
vi.mock("@/lib/api/remoteAccount", () => ({
  remoteAccountApi: {
    status: mocks.accountStatus,
    login: mocks.accountLogin,
    logout: mocks.accountLogout,
    createSocketTicket: mocks.socketTicket,
  },
}));
vi.mock("sonner", () => ({
  toast: {
    error: mocks.toastError,
    success: mocks.toastSuccess,
    warning: mocks.toastWarning,
  },
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
  send(payload: string) {
    this.sent.push(payload);
  }
  close() {
    this.closed = true;
  }
  open() {
    this.onopen?.();
  }
  message(payload: unknown) {
    this.onmessage?.({ data: JSON.stringify(payload) });
  }
}

async function login() {
  await screen.findByText("登录后自动完成 Codex 配置");
  fireEvent.change(screen.getByLabelText("账号"), {
    target: { value: "hang" },
  });
  fireEvent.change(screen.getByLabelText("密码"), {
    target: { value: "secret" },
  });
  fireEvent.click(screen.getByRole("button", { name: /登录并一键配置/ }));
  await waitFor(() => expect(mocks.configure).toHaveBeenCalledOnce());
}

describe("RemoteControlPage", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeWebSocket);
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
    FakeWebSocket.instances = [];
    mocks.listen.mockResolvedValue(mocks.unlisten);
    mocks.configurationStatus.mockResolvedValue({
      configured: false,
      configPath: "/test/.codex/config.toml",
      canRollback: false,
    });
    mocks.configure.mockResolvedValue({
      configured: true,
      model: "gpt-5.6-sol",
      models: ["gpt-5.6-sol", "gpt-5.6-terra"],
      configPath: "/test/.codex/config.toml",
      canRollback: true,
    });
    mocks.modelOptions.mockResolvedValue({
      defaultModel: "gpt-5.6-sol",
      models: ["gpt-5.6-sol", "gpt-5.6-terra"],
    });
    mocks.uploadDiagnosticBundle.mockResolvedValue({
      bundleId: "bundle-1",
      url: "https://diagnostics.example/bundle.zip?signature=test",
      expiresAt: 123456,
      expiresInSeconds: 604800,
    });
    mocks.copyText.mockResolvedValue(undefined);
    mocks.openExternal.mockResolvedValue(undefined);
    mocks.rollbackConfiguration.mockResolvedValue({
      configured: false,
      configPath: "/test/.codex/config.toml",
      canRollback: false,
    });
    mocks.remoteStatus.mockResolvedValue({ running: false });
    mocks.startRemote.mockResolvedValue({ running: true });
    mocks.stopRemote.mockResolvedValue({ running: false });
    mocks.send.mockResolvedValue(undefined);
    mocks.plusPlus.mockResolvedValue({ installed: false, summary: "optional" });
    mocks.launchPlusPlus.mockResolvedValue(undefined);
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: false,
    });
    mocks.accountLogin.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
      deviceId: "desktop-device-uuid",
      expiresAt: 123456,
    });
    mocks.accountLogout.mockResolvedValue({
      status: "ok",
      authenticated: false,
    });
    mocks.socketTicket.mockResolvedValue({
      status: "ok",
      websocketUrl:
        "ws://relay.test/api/codex-remote/account/ws?ticket=one-time-ticket",
      deviceId: "desktop-device-uuid",
    });
  });

  it("logs in and configures Codex without starting a process or remote bridge", async () => {
    render(<RemoteControlPage />);
    await login();

    expect(mocks.accountLogin).toHaveBeenCalledWith("hang", "secret");
    expect(mocks.configure).toHaveBeenCalledWith();
    expect(mocks.startRemote).not.toHaveBeenCalled();
    expect(mocks.socketTicket).not.toHaveBeenCalled();
    expect(FakeWebSocket.instances).toHaveLength(0);
    expect(
      await screen.findByText("配置已完成，现在启动 Codex"),
    ).toBeInTheDocument();
  });

  it("starts the optional phone bridge only after an explicit click", async () => {
    render(<RemoteControlPage />);
    await login();

    fireEvent.click(screen.getByRole("button", { name: /启用手机远程/ }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    expect(mocks.startRemote).toHaveBeenCalledWith();
    expect(mocks.socketTicket).toHaveBeenCalledOnce();
    const socket = FakeWebSocket.instances[0];
    expect(socket.url).toBe(
      "ws://relay.test/api/codex-remote/account/ws?ticket=one-time-ticket",
    );
    act(() => socket.open());
    act(() =>
      socket.message({ type: "relay.peer", role: "mobile", online: true }),
    );
    expect(await screen.findByText("手机已连接")).toBeInTheDocument();

    act(() => socket.message({ type: "relay.data", payload: '{"id":1}' }));
    await waitFor(() => expect(mocks.send).toHaveBeenCalledWith('{"id":1}'));
    const nativeListener = mocks.listen.mock.calls[0][1] as (event: {
      payload: string;
    }) => void;
    act(() => nativeListener({ payload: '{"method":"turn/completed"}' }));
    expect(JSON.parse(socket.sent[0])).toEqual({
      type: "relay.data",
      payload: '{"method":"turn/completed"}',
    });
  });

  it("restores the account without starting Codex or the remote bridge", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
      deviceId: "desktop-device-uuid",
      expiresAt: 123456,
    });
    render(<RemoteControlPage />);
    expect(await screen.findByText(/已登录 hang/)).toBeInTheDocument();
    expect(screen.queryByLabelText("密码")).not.toBeInTheDocument();
    expect(mocks.configure).not.toHaveBeenCalled();
    expect(mocks.startRemote).not.toHaveBeenCalled();
    expect(mocks.socketTicket).not.toHaveBeenCalled();
    expect(FakeWebSocket.instances).toHaveLength(0);
  });

  it("defaults to sol and applies a different account model from the selector", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    const user = userEvent.setup();
    render(<RemoteControlPage />);

    const selector = await screen.findByRole("combobox", { name: "默认模型" });
    await waitFor(() => expect(selector).toHaveTextContent("gpt-5.6-sol"));
    await user.click(selector);
    await user.click(
      await screen.findByRole("option", { name: "gpt-5.6-terra" }),
    );
    await user.click(screen.getByRole("button", { name: "立即一键配置" }));

    await waitFor(() =>
      expect(mocks.configure).toHaveBeenCalledWith("gpt-5.6-terra"),
    );
  });

  it("checks the bundled helper without launching Codex", async () => {
    mocks.plusPlus.mockResolvedValue({ installed: true, summary: "ready" });

    render(<RemoteControlPage />);

    await waitFor(() => expect(mocks.plusPlus).toHaveBeenCalledOnce());
    expect(mocks.launchPlusPlus).not.toHaveBeenCalled();
    expect(mocks.startRemote).not.toHaveBeenCalled();
  });

  it("stops the native app-server and account relay together", async () => {
    render(<RemoteControlPage />);
    await login();
    fireEvent.click(screen.getByRole("button", { name: /启用手机远程/ }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    fireEvent.click(screen.getByRole("button", { name: /停止手机远程/ }));
    await waitFor(() => expect(mocks.stopRemote).toHaveBeenCalledOnce());
    expect(socket.closed).toBe(true);
    expect(mocks.unlisten).toHaveBeenCalledOnce();
    expect(screen.getByText("未启用")).toBeInTheDocument();
    expect(screen.getByText("配置完成")).toBeInTheDocument();
  });

  it("keeps configuration successful when the optional CLI is missing", async () => {
    mocks.startRemote.mockRejectedValue(new Error("程序未找到"));
    render(<RemoteControlPage />);
    await login();

    fireEvent.click(screen.getByRole("button", { name: /启用手机远程/ }));

    await waitFor(() => expect(mocks.toastWarning).toHaveBeenCalled());
    expect(screen.getByText("配置完成")).toBeInTheDocument();
    expect(screen.getByText("手机连接异常")).toBeInTheDocument();
  });

  it("offers a one-click rollback after configuration", async () => {
    render(<RemoteControlPage />);
    await login();

    fireEvent.click(screen.getByRole("button", { name: "恢复配置" }));

    await waitFor(() =>
      expect(mocks.rollbackConfiguration).toHaveBeenCalledOnce(),
    );
    expect(screen.getByText("尚未配置")).toBeInTheDocument();
  });

  it("uses account login as the default instead of a pairing code", () => {
    render(<RemoteControlPage />);
    return waitFor(() => {
      expect(screen.getByText("登录后自动完成 Codex 配置")).toBeInTheDocument();
      expect(screen.queryByText("移动端配对码")).not.toBeInTheDocument();
    });
  });

  it("uploads a redacted diagnostic bundle and exposes its temporary link", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    render(<RemoteControlPage />);

    fireEvent.click(await screen.findByRole("button", { name: "上传诊断包" }));

    await waitFor(() =>
      expect(mocks.uploadDiagnosticBundle).toHaveBeenCalledOnce(),
    );
    expect(mocks.copyText).toHaveBeenCalledWith(
      "https://diagnostics.example/bundle.zip?signature=test",
    );
    fireEvent.click(screen.getByRole("button", { name: "打开链接" }));
    expect(mocks.openExternal).toHaveBeenCalledWith(
      "https://diagnostics.example/bundle.zip?signature=test",
    );
  });
});
