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
// particular would newly hijack in-app anchors across the whole window. The one exception is a RAW
// `mailto:` href, which goes to the mail app whatever its target.

import { act, cleanup, fireEvent, render, renderHook, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useExternalLinks } from "./useExternalLinks";
import { openUrl } from "./ipc";
import { Markdown } from "./markdown";
import {
  dismissLinkFailure,
  MAIL_APP_FAILED,
  showLinkFailure,
  useLinkFailure,
} from "./linkFailure";

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

afterEach(() => {
  cleanup();
  dismissLinkFailure();
  opened.mockReset();
  opened.mockImplementation(async () => undefined);
});

/** A click on a link whose open is refused with `error`, and what the link notice then says. */
async function noticeAfterFailedClick(attrs: Record<string, string>, error: unknown) {
  opened.mockImplementationOnce(() => Promise.reject(error));
  const notice = renderHook(() => useLinkFailure());
  const { anchor } = mountWithAnchor(attrs);
  await act(async () => {
    fireEvent.click(anchor);
  });
  return notice.result.current;
}

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

  it("leaves same-tab and other in-app links alone", () => {
    for (const href of ["/foo", "#section", "tel:+15550100"]) {
      const { anchor } = mountWithAnchor({ href });
      const ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
      anchor.dispatchEvent(ev);
      expect(ev.defaultPrevented).toBe(false);
    }
    expect(opened).not.toHaveBeenCalled();
  });

  it("hands a mail link to the mail app, whatever its target, and never lets it navigate", () => {
    // Markdown's mail links carry no target; PM's navigation guard refuses the default, so without
    // this the click did nothing.
    const links: Record<string, string>[] = [
      { href: "mailto:a@b.co" },
      { href: "MAILTO:a@b.co?subject=hi" },
      { href: "mailto:a@b.co", target: "_blank" },
    ];
    for (const attrs of links) {
      opened.mockClear();
      const { anchor } = mountWithAnchor(attrs);
      const ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
      anchor.dispatchEvent(ev);
      expect(opened).toHaveBeenCalledTimes(1);
      // The raw href: the backend opener keeps only the address.
      expect(opened).toHaveBeenCalledWith(attrs.href);
      expect(ev.defaultPrevented).toBe(true);
      cleanup();
    }
  });

  it("opens a Markdown mail link through the backend and never navigates PM's window", () => {
    renderHook(() => useExternalLinks());
    const view = render(<Markdown>{"Write to [Ann](mailto:ann@example.com)."}</Markdown>);
    const link = within(view.container).getByRole("link", { name: "Ann" });
    expect(link.getAttribute("target")).toBeNull();
    const ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    link.dispatchEvent(ev);
    expect(opened).toHaveBeenCalledWith("mailto:ann@example.com");
    expect(ev.defaultPrevented).toBe(true);
  });

  it("ignores a target=_blank anchor whose scheme is neither http(s) nor mailto", () => {
    // Fail-closed: `open_url` rejects these backend-side too, so this is the outer of two guards.
    for (const href of ["file:///etc/passwd", "//evil.example/x", "", " mailto:a@b.co"]) {
      const { anchor } = mountWithAnchor({ href, target: "_blank" });
      fireEvent.click(anchor);
    }
    expect(opened).not.toHaveBeenCalled();
  });

  it("ignores a non-primary button and an already-handled click", () => {
    for (const href of ["https://example.com", "mailto:a@b.co"]) {
      const { anchor } = mountWithAnchor({ href, target: "_blank" });
      anchor.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 2 }));
      expect(opened).not.toHaveBeenCalled();

      const handled = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
      handled.preventDefault();
      anchor.dispatchEvent(handled);
      expect(opened).not.toHaveBeenCalled();
    }
  });

  it("removes the listener on unmount", () => {
    const { hook, anchor } = mountWithAnchor({ href: "https://example.com", target: "_blank" });
    hook.unmount();
    fireEvent.click(anchor);
    expect(opened).not.toHaveBeenCalled();
  });

  // Review round 2: on a machine with no mail app (common on Linux) the click used to do nothing.
  it("says when the mail app didn't open, with the address to copy", async () => {
    expect(
      await noticeAfterFailedClick({ href: "mailto:ann@example.com?subject=hi" }, MAIL_APP_FAILED),
    ).toBe("PM couldn't open your mail app. The address is ann@example.com.");
  });

  it("says the backend's words for a mail or web link it refused", async () => {
    expect(
      await noticeAfterFailedClick(
        { href: "mailto:a%0Ab@example.com" },
        "That doesn't look like an email address.",
      ),
    ).toBe("That doesn't look like an email address.");
    cleanup();
    expect(
      await noticeAfterFailedClick(
        { href: "https://example.com", target: "_blank" },
        "Couldn't open the link (NotFound).",
      ),
    ).toBe("Couldn't open the link (NotFound).");
  });

  it("takes the last notice down on the next click, and leaves it down when that link opens", async () => {
    act(() => showLinkFailure("an earlier link didn't open"));
    const notice = renderHook(() => useLinkFailure());
    const { anchor } = mountWithAnchor({ href: "mailto:a@b.co" });
    await act(async () => {
      fireEvent.click(anchor);
    });
    expect(opened).toHaveBeenCalledWith("mailto:a@b.co");
    expect(notice.result.current).toBeNull();
  });

  it("says nothing for a link it leaves alone", () => {
    const notice = renderHook(() => useLinkFailure());
    opened.mockImplementation(() => Promise.reject(MAIL_APP_FAILED));
    const { anchor } = mountWithAnchor({ href: "/foo" });
    fireEvent.click(anchor);
    expect(notice.result.current).toBeNull();
  });
});
