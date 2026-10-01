// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A pointer to another section is its name, read from the one list the rail is built from — so a
// renamed section renames every pointer to it. Inside the tab it takes you there; outside one (a
// section rendered on its own) it is plain text, because a control that does nothing is worse than
// none.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { LocateProvider, SectionLink } from "./SectionLink";
import { sectionLabel } from "./sections";

afterEach(cleanup);

describe("SectionLink", () => {
  it("is the section's rail name, as a way there, inside the tab", () => {
    const locate = vi.fn();
    render(
      <LocateProvider locate={locate}>
        <p>
          Set it under <SectionLink to="sec-localai-roles" />.
        </p>
      </LocateProvider>,
    );
    const link = screen.getByRole("button", { name: sectionLabel("sec-localai-roles") });
    fireEvent.click(link);
    expect(locate).toHaveBeenCalledWith("sec-localai-roles");
  });

  it("is plain text outside one", () => {
    const { container } = render(
      <p>
        Set it under <SectionLink to="sec-localai-endpoint" />.
      </p>,
    );
    expect(screen.queryByRole("button")).toBeNull();
    expect(container.textContent).toBe(`Set it under ${sectionLabel("sec-localai-endpoint")}.`);
  });
});
