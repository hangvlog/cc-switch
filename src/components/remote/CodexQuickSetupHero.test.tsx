// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CodexQuickSetupHero } from "./CodexQuickSetupHero";

describe("CodexQuickSetupHero", () => {
  it("把一键配置作为明确的首要操作", async () => {
    const onConfigure = vi.fn();
    render(<CodexQuickSetupHero onConfigure={onConfigure} />);

    expect(
      screen.getByRole("heading", { name: "一键配置 Codex" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/无需手动填写 Base URL 或 API Key/),
    ).toBeInTheDocument();
    expect(screen.getByText(/不修改官方 Codex 配置与图标/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /立即一键配置/ }));
    expect(onConfigure).toHaveBeenCalledOnce();
  });
});
