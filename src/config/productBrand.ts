export const PRODUCT_NAME = "ClawKit Desktop";

const UPSTREAM_PRODUCT_NAME = "CC Switch";

/**
 * Keep upstream locale files untouched so upstream rebases stay small, while
 * presenting the independent ClawKit product name throughout the UI.
 */
export function applyProductBrand<T>(value: T): T {
  if (typeof value === "string") {
    return value.split(UPSTREAM_PRODUCT_NAME).join(PRODUCT_NAME) as T;
  }

  if (Array.isArray(value)) {
    return value.map((item) => applyProductBrand(item)) as T;
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        applyProductBrand(item),
      ]),
    ) as T;
  }

  return value;
}
