// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useFieldA11y } from "./Field";

afterEach(cleanup);

describe("useFieldA11y", () => {
  it("wires the label's htmlFor to the control id", () => {
    const { result } = renderHook(() => useFieldA11y());
    expect(result.current.labelProps.htmlFor).toBe(result.current.controlProps.id);
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
