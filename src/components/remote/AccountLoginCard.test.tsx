// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AccountLoginCard } from "./AccountLoginCard";

afterEach(cleanup);
describe("AccountLoginCard", () => {
  it("直接提交账号密码，清空密码并保持模型配置独立", async () => {
    const onLogin = vi.fn().mockResolvedValue(undefined);
    const onBrowserLogin = vi.fn();
    const user = userEvent.setup();
    render(<AccountLoginCard busy={null} onLogin={onLogin} onBrowserLogin={onBrowserLogin} />);
    await user.type(screen.getByLabelText("账号"), " person@example.test ");
    await user.type(screen.getByLabelText("密码"), "test-password");
    await user.keyboard("{Enter}");
    expect(onLogin).toHaveBeenCalledWith("person@example.test", "test-password");
    expect(onBrowserLogin).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByLabelText("密码")).toHaveValue(""));
    expect(screen.getByText("登录不会修改现有 Codex 模型配置")).toBeInTheDocument();
  });
  it("保留独立浏览器登录入口，并防止登录期间重复提交", async () => {
    const onLogin = vi.fn();
    const onBrowserLogin = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    const { rerender } = render(<AccountLoginCard busy={null} onLogin={onLogin} onBrowserLogin={onBrowserLogin} />);
    await user.click(screen.getByRole("button", { name: "使用浏览器登录" }));
    expect(onBrowserLogin).toHaveBeenCalledOnce();
    expect(onLogin).not.toHaveBeenCalled();
    rerender(<AccountLoginCard busy="browser" onLogin={onLogin} onBrowserLogin={onBrowserLogin} />);
    expect(screen.getByLabelText("账号")).toBeDisabled();
    expect(screen.getByRole("button", { name: "等待浏览器完成登录…" })).toBeDisabled();
  });
});
