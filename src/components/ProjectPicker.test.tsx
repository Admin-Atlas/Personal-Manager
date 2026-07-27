// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The rules that make the project field different from the tag field. Each of these is a decision
// that is invisible in the markup and expensive to get wrong: a document with no project is not a
// state the store has, a project name is not a lowercase label, and a comma is a legal character in
// a real company name.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LinkedBadge, ProjectPicker, ProjectSummary, projectsOf } from "./ProjectPicker";

// This project has no global auto-cleanup, so renders accumulate in one jsdom document and every
// query after the first would match several nodes.
afterEach(cleanup);

function setup(value: string[]) {
  const onChange = vi.fn();
  render(<ProjectPicker value={value} onChange={onChange} suggestions={["Sales", "Marketing"]} />);
  return onChange;
}

describe("ProjectPicker", () => {
  it("the primary project has no remove control, so a document can never end up with none", () => {
    setup(["Sales"]);
    expect(screen.queryByLabelText("Unlink from Sales")).toBeNull();
    // It is still labelled, so the user can see WHICH one is primary.
    expect(screen.getAllByText(/Primary/).length).toBeGreaterThan(0);
  });

  it("a linked project can be unlinked, and the primary survives it", () => {
    const onChange = setup(["Sales", "Marketing"]);
    fireEvent.click(screen.getByLabelText("Unlink from Marketing"));
    expect(onChange).toHaveBeenCalledWith(["Sales"]);
  });

  it("keeps the casing the user typed — downcasing would stop a name resolving to its project", () => {
    const onChange = setup(["Sales"]);
    const input = screen.getByLabelText("Add a project");
    fireEvent.change(input, { target: { value: "Atlas, Inc." } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith(["Sales", "Atlas, Inc."]);
  });

  it("does not add a project the document is already in, however it is cased", () => {
    const onChange = setup(["Sales"]);
    const input = screen.getByLabelText("Add a project");
    fireEvent.change(input, { target: { value: "sales" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).not.toHaveBeenCalled();
  });

  // Enter commits; a comma does NOT, because it belongs inside the name.
  it("a comma is typed into the name rather than committing it", () => {
    const onChange = setup(["Sales"]);
    const input = screen.getByLabelText("Add a project");
    fireEvent.change(input, { target: { value: "Atlas," } });
    fireEvent.keyDown(input, { key: "," });
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("membership display", () => {
  it("a single-project document reads exactly as it always did", () => {
    render(<ProjectSummary doc={{ project: "Sales", linked_projects: [] }} />);
    expect(screen.getByText("Sales")).toBeTruthy();
    expect(screen.queryByText(/more project/)).toBeNull();
  });

  it("a multi-project document says so, and names them all on hover", () => {
    render(<ProjectSummary doc={{ project: "Sales", linked_projects: ["Marketing", "Ops"] }} />);
    expect(screen.getByText(/\+2 more projects/)).toBeTruthy();
    expect(screen.getByTitle("Sales, Marketing, Ops")).toBeTruthy();
  });

  it("the linked badge names the document's real home, so the user can go and change it", () => {
    render(<LinkedBadge home="Sales" />);
    expect(screen.getByTitle(/primary project is Sales/)).toBeTruthy();
  });

  it("projectsOf puts the primary first — the order the backend reads it in", () => {
    expect(projectsOf({ project: "Sales", linked_projects: ["Ops"] })).toEqual(["Sales", "Ops"]);
  });
});
