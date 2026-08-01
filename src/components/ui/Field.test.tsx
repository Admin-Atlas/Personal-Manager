// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cleanup, render, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Field, useFieldA11y } from "./Field";

afterEach(cleanup);

describe("useFieldA11y", () => {
  it("wires the label's htmlFor to the control id", () => {
    const { result } = renderHook(() => useFieldA11y());
    expect(result.current.labelProps.htmlFor).toBe(result.current.controlProps.id);
  });

  it("points the control's aria-labelledby at the label's own id", () => {
    // The second half of the shared core: `htmlFor` reaches a labelable element and nothing else,
    // so the switch and group controls in Settings need this pair to be named at all.
    const { result } = renderHook(() => useFieldA11y());
    expect(result.current.controlProps["aria-labelledby"]).toBe(result.current.labelProps.id);
    expect(result.current.labelProps.id).not.toBe(result.current.controlProps.id);
  });

  it("marks the control invalid and describes it by the error when an error is present", () => {
    const { result } = renderHook(() => useFieldA11y({ error: "That doesn't match" }));
    expect(result.current.controlProps["aria-invalid"]).toBe(true);
    expect(result.current.controlProps["aria-describedby"]).toBe(result.current.errorProps.id);
    expect(result.current.errorProps.role).toBe("alert");
  });

  it("sets no aria-invalid / aria-describedby when there is no error or description", () => {
    const { result } = renderHook(() => useFieldA11y());
    expect(result.current.controlProps["aria-invalid"]).toBeUndefined();
    expect(result.current.controlProps["aria-describedby"]).toBeUndefined();
  });

  it("composes description + error ids into aria-describedby", () => {
    const { result } = renderHook(() => useFieldA11y({ error: "e", description: "d" }));
    const describedBy = result.current.controlProps["aria-describedby"] ?? "";
    expect(describedBy).toContain(result.current.descriptionProps.id);
    expect(describedBy).toContain(result.current.errorProps.id);
  });
});

describe("Field", () => {
  // The cases above exercise the hook's return SHAPE. This one proves the ids actually resolve once
  // rendered — the accessible name is what a screen reader computes, not what the object holds.
  it("names its control by the visible label", () => {
    const { getByLabelText } = render(
      <Field label="Passphrase">{(controlProps) => <input {...controlProps} />}</Field>,
    );
    expect(getByLabelText("Passphrase")).toBeTruthy();
  });

  it("describes its control by the description", () => {
    const { getByLabelText } = render(
      <Field label="Passphrase" description="At least 12 characters.">
        {(controlProps) => <input {...controlProps} />}
      </Field>,
    );
    const input = getByLabelText("Passphrase");
    const describedBy = input.getAttribute("aria-describedby");
    expect(document.getElementById(describedBy!)?.textContent).toBe("At least 12 characters.");
  });
});

// Eleven sites use the HOOK with a sibling `<label>` rather than the component, because `Field`
// hard-codes `text-sm text-ink2` and a `gap-1` column while those sites carry their own type scale,
// bespoke `mt-3`/`mt-4` spacing and trailing links. The cases above prove `Field`'s layout wires up;
// this proves the shape those eleven actually emit does — a label and a control as plain siblings,
// associated by nothing but the two spreads.
describe("useFieldA11y spread over sibling elements", () => {
  function SiblingPair({ error }: { error?: string }) {
    const a11y = useFieldA11y({ error });
    return (
      <div>
        <label {...a11y.labelProps} className="block text-sm font-medium text-ink2">
          OpenRouter API key
        </label>
        <input {...a11y.controlProps} type="password" placeholder="sk-or-..." />
        {error && <p {...a11y.errorProps}>{error}</p>}
      </div>
    );
  }

  it("names the control by the label rather than by the placeholder", () => {
    const { getByLabelText, queryByLabelText } = render(<SiblingPair />);
    expect(getByLabelText("OpenRouter API key")).toBeTruthy();
    // The defect this closes: with no association the placeholder was the last-resort name, and it
    // disappeared on the first keystroke.
    expect(queryByLabelText("sk-or-...")).toBeNull();
  });

  it("announces a validation error and points the control at it", () => {
    const { getByLabelText, getByRole } = render(<SiblingPair error="Couldn't read that" />);
    const input = getByLabelText("OpenRouter API key");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(getByRole("alert").id).toBe(input.getAttribute("aria-describedby"));
  });

  it("mints distinct ids for two pairs on the same surface", () => {
    const { getAllByLabelText } = render(
      <>
        <SiblingPair />
        <SiblingPair />
      </>,
    );
    const [first, second] = getAllByLabelText("OpenRouter API key");
    expect(first.id).not.toBe(second.id);
  });
});
