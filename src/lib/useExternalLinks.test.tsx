// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The app-wide external-link interceptor. It had no test at all while it was an inline effect in
// App, which is part of why a second webview root shipped without it (see PopoverRoot.test.tsx).
//
// What is worth pinning is the SHAPE of the guard, because every condition in it is doing security
// work: it fires only on a primary-button click that nothing else has claimed, only on an anchor
// explicitly marked `target="_blank"`, and only when the RAW href starts `http://`/`https://`. Each
// of those is the kind of condition a refactor "simplifies" away, and dropping the target test in
// particular would newly hijack in-app anchors across the whole window.

import { cleanup, fireEvent, render, renderHook, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useExternalLinks } from "./useExternalLinks";
import { openUrl } from "./ipc";

vi.mock("./ipc", () => ({
  openUrl: vi.fn(async () => undefined),
}));

const opened = vi.mocked(openUrl);

/** Mount the hook and put an anchor in the document to click. Queries are scoped to this render's
 *  own container, so a case that mounts two anchors doesn't pick up the wrong one. */
function mountWithAnchor(attrs: Record<string, string>) {
  const hook = renderHook(() => useExternalLinks());
  const view = render(
    <a {...attrs} data-testid="link">
      link
    </a>,
  );
  return { hook, anchor: within(view.container).getByTestId("link") };
}

beforeEach(() => {
  opened.mockClear();
});

afterEach(cleanup);

describe("useExternalLinks", () => {
  it("hands a target=_blank http(s) link to the OS browser and prevents the default", () => {
    const { anchor } = mountWithAnchor({ href: "https://example.com/x", target: "_blank" });
    const ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(ev);
    expect(opened).toHaveBeenCalledTimes(1);
    expect(opened).toHaveBeenCalledWith("https://example.com/x");
    // Without this the webview would ALSO try to act on the link.
    expect(ev.defaultPrevented).toBe(true);
  });

  it("finds the anchor from a click on something nested inside it", () => {
    renderHook(() => useExternalLinks());
    const view = render(
      <a href="http://example.com" target="_blank">
        <span data-testid="inner">deep</span>
      </a>,
    );
    fireEvent.click(within(view.container).getByTestId("inner"));
    // Markdown links wrap `<code>`/`<strong>`; the handler has to walk up, or those links go dead.
    expect(opened).toHaveBeenCalledWith("http://example.com");
  });

  it("leaves same-tab and mailto links alone", () => {
    const { anchor } = mountWithAnchor({ href: "/foo" });
    fireEvent.click(anchor);
    expect(opened).not.toHaveBeenCalled();

    const mail = mountWithAnchor({ href: "mailto:a@b.co" });
    fireEvent.click(mail.anchor);
    expect(opened).not.toHaveBeenCalled();
  });

  it("ignores a target=_blank anchor whose scheme is not http(s)", () => {
    // Fail-closed: `open_url` rejects these backend-side too, so this is the outer of two guards.
    for (const href of ["mailto:a@b.co", "file:///etc/passwd", "//evil.example/x", ""]) {
      const { anchor } = mountWithAnchor({ href, target: "_blank" });
      fireEvent.click(anchor);
    }
    expect(opened).not.toHaveBeenCalled();
  });

  it("ignores a non-primary button and an already-handled click", () => {
    const { anchor } = mountWithAnchor({ href: "https://example.com", target: "_blank" });
    anchor.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 2 }));
    expect(opened).not.toHaveBeenCalled();

    const handled = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    handled.preventDefault();
    anchor.dispatchEvent(handled);
    expect(opened).not.toHaveBeenCalled();
  });

  it("removes the listener on unmount", () => {
    const { hook, anchor } = mountWithAnchor({ href: "https://example.com", target: "_blank" });
    hook.unmount();
    fireEvent.click(anchor);
    expect(opened).not.toHaveBeenCalled();
  });
});
