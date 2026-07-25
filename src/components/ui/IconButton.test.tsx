// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// IconButton's contract: the required label becomes both the accessible name and the default tooltip,
// it always carries the --tap-min target floor (WCAG 2.5.8), and it defaults to type="button".

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { IconButton } from "./IconButton";

afterEach(cleanup);

describe("IconButton", () => {
  it("uses label as the accessible name and the default tooltip", () => {
    const { getByRole } = render(<IconButton label="Remove">×</IconButton>);
    const btn = getByRole("button", { name: "Remove" });
    expect(btn.getAttribute("title")).toBe("Remove");
    expect(btn.getAttribute("type")).toBe("button");
  });

  it("carries the tap-target floor and can be disabled", () => {
    const { getByRole } = render(
      <IconButton label="Close" disabled>
        ×
      </IconButton>,
    );
    const btn = getByRole("button", { name: "Close" });
    expect(btn.className).toContain("min-h-[var(--tap-min,24px)]");
    expect(btn.className).toContain("min-w-[var(--tap-min,24px)]");
    expect((btn as HTMLButtonElement).disabled).toBe(true);
  });

  it("lets an explicit title override the label tooltip", () => {
    const { getByRole } = render(
      <IconButton label="Delete" title="Delete this item permanently">
        ×
      </IconButton>,
    );
    expect(getByRole("button").getAttribute("title")).toBe("Delete this item permanently");
  });
});
