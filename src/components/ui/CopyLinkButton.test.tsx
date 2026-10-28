// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// CopyLinkButton's contract: it puts exactly its link on the clipboard, says so for a moment and
// then reads "Copy link" again, and a webview that has no clipboard or refuses the write leaves it
// quietly as it was.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { CopyLinkButton } from "./CopyLinkButton";

const URL_ = "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&state=y";

function clipboard(writeText: ((text: string) => Promise<void>) | undefined) {
  Object.defineProperty(navigator, "clipboard", {
    value: writeText ? { writeText } : undefined,
    configurable: true,
  });
}

afterEach(() => {
  cleanup();
  clipboard(undefined);
  vi.useRealTimers();
});

describe("CopyLinkButton", () => {
  it("copies its link, says so, and reads Copy link again after a moment", async () => {
    vi.useFakeTimers();
    const writeText = vi.fn().mockResolvedValue(undefined);
    clipboard(writeText);
    const { getByRole } = render(<CopyLinkButton url={URL_} />);
    await act(async () => {
      fireEvent.click(getByRole("button", { name: "Copy link" }));
    });
    expect(writeText).toHaveBeenCalledWith(URL_);
    expect(getByRole("button").textContent).toBe("Copied ✓");
    act(() => {
      vi.advanceTimersByTime(1500);
    });
    expect(getByRole("button").textContent).toBe("Copy link");
  });

  it("a refused write, or no clipboard at all, leaves it reading Copy link", async () => {
    clipboard(vi.fn().mockRejectedValue(new Error("not allowed")));
    const { getByRole, rerender } = render(<CopyLinkButton url={URL_} />);
    await act(async () => {
      fireEvent.click(getByRole("button"));
    });
    expect(getByRole("button").textContent).toBe("Copy link");
    clipboard(undefined);
    rerender(<CopyLinkButton url={URL_} />);
    await act(async () => {
      fireEvent.click(getByRole("button"));
    });
    expect(getByRole("button").textContent).toBe("Copy link");
  });

  it("is a small tertiary button unless asked otherwise", () => {
    const { getByRole, rerender } = render(<CopyLinkButton url={URL_} />);
    expect(getByRole("button").dataset.variant).toBe("tertiary");
    rerender(<CopyLinkButton url={URL_} variant="secondary" />);
    expect(getByRole("button").dataset.variant).toBe("secondary");
  });
});
