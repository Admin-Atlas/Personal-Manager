// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The chat honesty strip (#297) + its copy map. Pins that every backend fallback slug maps to plain
// text (and an unknown slug degrades rather than leaking the raw token), that the cloud model is
// named, and that Dismiss fires.

import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FallbackStrip, fallbackCopy } from "./FallbackStrip";

describe("fallbackCopy", () => {
  it("maps the known slugs to friendly reasons", () => {
    expect(fallbackCopy("cooldown")).toContain("resting");
    expect(fallbackCopy("hard_failure:timeout")).toContain("timed out");
    expect(fallbackCopy("hard_failure:refused")).toContain("reachable");
    expect(fallbackCopy("hard_failure:model_loading")).toContain("loading");
    expect(fallbackCopy("hard_failure:reply_too_large")).toContain("too large");
    expect(fallbackCopy("hard_failure:degenerate_stream")).toContain("unusable");
  });

  // The banked half of a contract whose other half is compiler-enforced. `FallbackReason::PowerPolicy`
  // (llm_gateway.rs) has no producer yet and carries "Do not remove it or 'clean up' the unused
  // variant" — in Rust, exhaustiveness makes that stick. It does NOT survive the IPC string boundary:
  // delete the `power_policy` branch in FallbackStrip.tsx as apparently-dead code and the slug falls
  // through to the generic clause, reporting a DELIBERATE power-saving switch as a local model
  // FAILURE — with the whole suite still green, because the test below actively blesses that output
  // for unrecognised slugs. This is the guard that makes it fail instead.
  it("never reports a deliberate power-policy switch as a failure", () => {
    expect(fallbackCopy("power_policy")).toContain("save power");
    expect(fallbackCopy("power_policy")).not.toContain("couldn't answer");
  });

  it("degrades an unknown slug to a generic reason (never leaks the raw token)", () => {
    const copy = fallbackCopy("some_future_reason");
    expect(copy).toContain("couldn't answer");
    expect(copy).not.toContain("some_future_reason");
  });
});

describe("FallbackStrip", () => {
  it("renders the reason + the cloud model and fires onDismiss", () => {
    const onDismiss = vi.fn();
    const { container, getByTitle } = render(
      <FallbackStrip
        fallback={{
          from_model: "llama3",
          to_model: "openai/gpt-x",
          reason: "hard_failure:timeout",
        }}
        onDismiss={onDismiss}
      />,
    );
    expect(container.textContent).toContain("timed out");
    expect(container.textContent).toContain("gpt-x");
    fireEvent.click(getByTitle("Dismiss"));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("degrades gracefully when the model names are empty", () => {
    const { container } = render(
      <FallbackStrip
        fallback={{ from_model: "", to_model: "", reason: "cooldown" }}
        onDismiss={() => {}}
      />,
    );
    expect(container.textContent).toContain("cloud");
    expect(container.textContent).not.toContain("(via");
  });
});
