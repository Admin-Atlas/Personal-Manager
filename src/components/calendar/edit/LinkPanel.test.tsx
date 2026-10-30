// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description's link panel (C19 §7.4): an address is read as typed before it reaches the
// description, Escape closes only the panel, and Remove link is there only on a link.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { LinkPanel, NOT_AN_ADDRESS } from "./LinkPanel";

afterEach(cleanup);

function panel(over: { text?: string; href?: string; editing?: boolean } = {}) {
  const onApply = vi.fn<(text: string, href: string) => void>();
  const onRemove = vi.fn();
  const onCancel = vi.fn();
  render(
    <LinkPanel
      text={over.text ?? "the site"}
      href={over.href ?? ""}
      editing={over.editing ?? false}
      onApply={onApply}
      onRemove={onRemove}
      onCancel={onCancel}
    />,
  );
  return { onApply, onRemove, onCancel };
}

const link = () => screen.getByLabelText<HTMLInputElement>("Link");
const text = () => screen.getByLabelText<HTMLInputElement>("Text");

describe("LinkPanel", () => {
  it.each([
    ["example.com", "https://example.com"],
    ["  www.example.org/a?b=1  ", "https://www.example.org/a?b=1"],
    ["name@example.com", "mailto:name@example.com"],
    ["https://example.com/x", "https://example.com/x"],
  ])("an_address_is_checked_before_it_is_applied: %j", (typed, address) => {
    const { onApply } = panel();
    fireEvent.change(link(), { target: { value: typed } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApply).toHaveBeenCalledExactlyOnceWith("the site", address);
  });

  it.each(["javascript:x", "//x", "two words", "", "file:///etc/passwd", "data:text/html,x"])(
    "an_address_that_isnt_one_is_refused: %j",
    (typed) => {
      const { onApply } = panel({ href: "https://example.com" });
      fireEvent.change(link(), { target: { value: typed } });
      link().focus();
      fireEvent.click(screen.getByRole("button", { name: "Apply" }));
      expect(onApply).not.toHaveBeenCalled();
      const said = screen.getByRole("alert");
      expect(said.textContent).toBe(NOT_AN_ADDRESS);
      // Said under the field, and named as its error; focus stays there.
      expect(link().getAttribute("aria-describedby")).toBe(said.id);
      expect(link().getAttribute("aria-invalid")).toBe("true");
      expect(document.activeElement).toBe(link());
      // Typing again takes the line away.
      fireEvent.change(link(), { target: { value: "example.com" } });
      expect(screen.queryByRole("alert")).toBeNull();
    },
  );

  it("enter_in_either_field_applies", () => {
    const { onApply } = panel({ text: "" });
    fireEvent.change(text(), { target: { value: "PM" } });
    fireEvent.change(link(), { target: { value: "example.com" } });
    expect(fireEvent.keyDown(text(), { key: "Enter" })).toBe(false);
    expect(onApply).toHaveBeenLastCalledWith("PM", "https://example.com");
    fireEvent.keyDown(link(), { key: "Enter" });
    expect(onApply).toHaveBeenCalledTimes(2);
  });

  it("escape_closes_only_the_panel", () => {
    const { onCancel } = panel();
    const seen = vi.fn();
    window.addEventListener("keydown", seen);
    try {
      fireEvent.keyDown(link(), { key: "Escape" });
    } finally {
      window.removeEventListener("keydown", seen);
    }
    expect(onCancel).toHaveBeenCalledTimes(1);
    // The editor's dialog listens on the window: it never hears this Escape.
    expect(seen).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("remove_link_is_offered_only_on_a_link", () => {
    expect(panel().onRemove).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Remove link" })).toBeNull();
    cleanup();
    const { onRemove } = panel({ editing: true, href: "https://example.com" });
    expect(link().value).toBe("https://example.com");
    fireEvent.click(screen.getByRole("button", { name: "Remove link" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("labels_name_their_fields", () => {
    panel({ text: "the site", href: "https://example.com" });
    expect(text().value).toBe("the site");
    expect(link().value).toBe("https://example.com");
    // The address is what the panel is for: it takes focus.
    expect(document.activeElement).toBe(link());
    // One primary button on screen is the editor's Save: Apply is drawn as a secondary one.
    const apply = screen.getByRole("button", { name: "Apply" }).className.split(" ");
    expect(apply).toContain("border-border2");
    expect(apply).not.toContain("bg-accent");
  });
});
