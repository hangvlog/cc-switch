import { describe, expect, it } from "vitest";

import { applyProductBrand, PRODUCT_NAME } from "./productBrand";

describe("product branding", () => {
  it("rebrands nested user-visible strings without mutating the source", () => {
    const source = {
      title: "CC Switch",
      nested: ["Welcome to CC Switch", { path: "~/.cc-switch" }],
    };

    const branded = applyProductBrand(source);

    expect(branded).toEqual({
      title: PRODUCT_NAME,
      nested: [`Welcome to ${PRODUCT_NAME}`, { path: "~/.cc-switch" }],
    });
    expect(source.title).toBe("CC Switch");
  });
});
