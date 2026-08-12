// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Header } from "../Header";

describe("Header", () => {
  it("exposes only the non-interactive header surface as a Tauri drag region", () => {
    const { container } = render(
      <Header variant="dashboard" onRefresh={() => undefined} onOpenSettings={() => undefined} />,
    );

    expect(container.querySelector(".header")).toHaveAttribute("data-tauri-drag-region");
    expect(container.querySelector(".header__brand")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByRole("button", { name: "Refresh quotas" })).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
    expect(screen.getByRole("button", { name: "Open settings" })).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
  });
});
