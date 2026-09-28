import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  ownerStatus: vi.fn(),
  ownerCapabilities: vi.fn(),
  enableOwner: vi.fn(),
  launchOriginal: vi.fn(),
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
    ownerStatus: mocks.ownerStatus,
    ownerCapabilities: mocks.ownerCapabilities,
    enableOwner: mocks.enableOwner,
    launchOriginal: mocks.launchOriginal,
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
import {
  COMPATIBILITY_CODEX_ENDPOINT,
  SECURE_CODEX_ENDPOINT,
} from "@/components/remote/CodexEndpointSelector";

async function login() {
  await screen.findByLabelText("账号");
  fireEvent.change(screen.getByLabelText("账号"), {
    target: { value: "hang" },
  });
  fireEvent.change(screen.getByLabelText("密码"), {
    target: { value: "secret" },
  });
  fireEvent.click(screen.getByRole("button", { name: "登录" }));
  await screen.findByText("手机接续 Codex 原任务");
}

async function openConfiguration() {
  fireEvent.click(await screen.findByText("可选：使用 ClawKit 模型服务"));
}

describe("RemoteControlPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.ownerStatus.mockResolvedValue({ enabled: false, phase: "disabled" });
    mocks.ownerCapabilities.mockResolvedValue({
      build: "26.924.22138:11645",
      canSend: true,
    });
    mocks.enableOwner.mockImplementation(async (enabled: boolean) => ({
      enabled,
      phase: enabled ? "waiting" : "disabled",
    }));
    mocks.launchOriginal.mockResolvedValue(undefined);
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: vi.fn(),
    });
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
      baseUrl: COMPATIBILITY_CODEX_ENDPOINT,
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

  it("logs in without rewriting Codex configuration or launching a bridge", async () => {
    render(<RemoteControlPage />);
    await login();
    expect(mocks.accountLogin).toHaveBeenCalledWith("hang", "secret");
    expect(mocks.configure).not.toHaveBeenCalled();
    expect(mocks.enableOwner).not.toHaveBeenCalled();
    expect(mocks.startRemote).not.toHaveBeenCalled();
  });
  it("enables native owner remote without model setup and survives navigation", async () => {
    const rendered = render(<RemoteControlPage />);
    await login();
    fireEvent.click(screen.getByRole("button", { name: "启用手机远程" }));
    await screen.findByText("等待同账号手机");
    expect(mocks.enableOwner).toHaveBeenCalledWith(true);
    expect(mocks.configure).not.toHaveBeenCalled();
    expect(mocks.socketTicket).not.toHaveBeenCalled();
    rendered.unmount();
    expect(mocks.enableOwner).not.toHaveBeenCalledWith(false);
  });
  it("stops the native bridge explicitly", async () => {
    render(<RemoteControlPage />);
    await login();
    fireEvent.click(screen.getByRole("button", { name: "启用手机远程" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "停止手机远程" }),
    );
    await waitFor(() =>
      expect(mocks.enableOwner).toHaveBeenLastCalledWith(false),
    );
  });
  it("opens ordinary Codex without the Codex++ launcher", async () => {
    render(<RemoteControlPage />);
    await login();
    fireEvent.click(screen.getByRole("button", { name: "打开 Codex" }));
    await waitFor(() => expect(mocks.launchOriginal).toHaveBeenCalledOnce());
    expect(mocks.launchPlusPlus).not.toHaveBeenCalled();
  });

  it("defaults to sol and applies a different account model from the selector", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    const user = userEvent.setup();
    render(<RemoteControlPage />);
    await openConfiguration();

    const selector = await screen.findByRole("combobox", { name: "默认模型" });
    await waitFor(() => expect(selector).toHaveTextContent("gpt-5.6-sol"));
    await user.click(selector);
    await user.click(
      await screen.findByRole("option", { name: "gpt-5.6-terra" }),
    );
    await user.click(screen.getByRole("button", { name: "立即一键配置" }));

    await waitFor(() =>
      expect(mocks.configure).toHaveBeenCalledWith(
        "gpt-5.6-terra",
        COMPATIBILITY_CODEX_ENDPOINT,
      ),
    );
  });

  it("offers HTTP by default and can apply the HTTPS endpoint", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    const user = userEvent.setup();
    render(<RemoteControlPage />);
    await openConfiguration();

    const selector = await screen.findByRole("combobox", {
      name: "模型服务地址",
    });
    expect(selector).toHaveTextContent("兼容 HTTP（默认）");
    expect(screen.getByText(COMPATIBILITY_CODEX_ENDPOINT)).toBeInTheDocument();

    await user.click(selector);
    await user.click(await screen.findByRole("option", { name: "安全 HTTPS" }));
    await user.click(screen.getByRole("button", { name: "立即一键配置" }));

    await waitFor(() =>
      expect(mocks.configure).toHaveBeenCalledWith(
        "gpt-5.6-sol",
        SECURE_CODEX_ENDPOINT,
      ),
    );
  });

  it("restores and applies a custom HTTP or HTTPS endpoint", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    mocks.configurationStatus.mockResolvedValue({
      configured: true,
      model: "gpt-5.6-sol",
      models: ["gpt-5.6-sol"],
      baseUrl: "https://custom.example/v1",
      configPath: "/test/.codex/config.toml",
      canRollback: true,
    });
    const user = userEvent.setup();
    render(<RemoteControlPage />);
    await openConfiguration();

    const selector = await screen.findByRole("combobox", {
      name: "模型服务地址",
    });
    await waitFor(() => expect(selector).toHaveTextContent("自定义地址"));
    const input = screen.getByLabelText("自定义 HTTP/HTTPS 地址");
    expect(input).toHaveValue("https://custom.example/v1");

    await user.clear(input);
    await user.type(input, "http://10.0.0.8:8080/v1/");
    await user.click(screen.getByRole("button", { name: "应用所选配置" }));

    await waitFor(() =>
      expect(mocks.configure).toHaveBeenCalledWith(
        "gpt-5.6-sol",
        "http://10.0.0.8:8080/v1/",
      ),
    );
  });

  it("offers the new compatibility endpoint when reopening a legacy IP configuration", async () => {
    mocks.accountStatus.mockResolvedValue({
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    mocks.configurationStatus.mockResolvedValue({
      configured: true,
      model: "gpt-5.6-sol",
      models: ["gpt-5.6-sol"],
      baseUrl: "http://62.234.99.177:80/v1",
      configPath: "/test/.codex/config.toml",
      canRollback: true,
    });
    render(<RemoteControlPage />);
    await openConfiguration();
    const applyButton = await screen.findByRole("button", {
      name: "应用所选配置",
    });
    expect(mocks.configure).not.toHaveBeenCalled();
    fireEvent.click(applyButton);
    await waitFor(() =>
      expect(mocks.configure).toHaveBeenCalledWith(
        "gpt-5.6-sol",
        COMPATIBILITY_CODEX_ENDPOINT,
      ),
    );
  });

  it("does not apply an invalid custom endpoint", async () => {
    mocks.accountStatus.mockResolvedValue({
      status: "ok",
      authenticated: true,
      user: { id: 7, username: "hang" },
    });
    const user = userEvent.setup();
    render(<RemoteControlPage />);
    await openConfiguration();

    const selector = await screen.findByRole("combobox", {
      name: "模型服务地址",
    });
    await user.click(selector);
    await user.click(await screen.findByRole("option", { name: "自定义地址" }));
    const input = screen.getByLabelText("自定义 HTTP/HTTPS 地址");
    await user.type(input, "ftp://invalid.example/v1");

    expect(screen.getByRole("button", { name: "立即一键配置" })).toBeDisabled();
    expect(mocks.configure).not.toHaveBeenCalled();
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
