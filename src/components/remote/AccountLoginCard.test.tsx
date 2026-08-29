// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AccountLoginCard } from "./AccountLoginCard";

describe("AccountLoginCard", () => {
  it("用账号密码触发登录并一键配置", async () => {
    const onLogin = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<AccountLoginCard busy={false} onLogin={onLogin} />);

    await user.type(screen.getByLabelText("账号"), "15738079480");
    await user.type(screen.getByLabelText("密码"), "test-password");
    await user.click(screen.getByRole("button", { name: /登录并一键配置/ }));

    expect(onLogin).toHaveBeenCalledWith("15738079480", "test-password");
    expect(screen.getByText(/不会覆盖官方 Codex 配置/)).toBeInTheDocument();
  });
});
