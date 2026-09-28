// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CodexQuickSetupHero } from "./CodexQuickSetupHero";

describe("CodexQuickSetupHero", () => {
  it("把原任务接续作为首要操作并明确模型配置可选", async () => {
    const onConfigure = vi.fn();
    render(<CodexQuickSetupHero onConfigure={onConfigure} />);

    expect(
      screen.getByRole("heading", { name: "用手机接续 Codex" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/模型服务配置为可选项/),
    ).toBeInTheDocument();
    expect(screen.getByText(/沿用原任务的模型、目录和权限/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /连接 Codex 与手机/ }));
    expect(onConfigure).toHaveBeenCalledOnce();
  });
});
