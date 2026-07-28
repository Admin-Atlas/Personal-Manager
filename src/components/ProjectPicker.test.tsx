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
  it("the only project has no remove control, so a document can never end up with none", () => {
    setup(["Sales"]);
    expect(screen.queryByLabelText(/Unlink from Sales|Remove Sales/)).toBeNull();
    // It is still labelled, so the user can see WHICH one is primary.
    expect(screen.getAllByText(/Primary/).length).toBeGreaterThan(0);
  });

  it("a linked project can be unlinked, and the primary survives it", () => {
    const onChange = setup(["Sales", "Marketing"]);
    fireEvent.click(screen.getByLabelText("Unlink from Marketing"));
    expect(onChange).toHaveBeenCalledWith(["Sales"]);
  });

  // Changing where a document is really filed. The picker only reorders — `value[0]` is the home,
  // and both call sites write the list back as `[project, ...also_projects]` — so these assertions
  // are about ORDER, which is the whole mechanism.
  describe("changing the primary", () => {
    it("promotes a linked project when its name is clicked", () => {
      const onChange = setup(["Sales", "Marketing"]);
      fireEvent.click(screen.getByLabelText("Make Marketing the primary project"));
      expect(onChange).toHaveBeenCalledWith(["Marketing", "Sales"]);
    });

    it("keeps the others, in order, when promoting from the middle", () => {
      const onChange = setup(["Sales", "Marketing", "Ops"]);
      fireEvent.click(screen.getByLabelText("Make Ops the primary project"));
      expect(onChange).toHaveBeenCalledWith(["Ops", "Sales", "Marketing"]);
    });

    // What Bobby expected to already happen, and the reason the primary carries an × at all.
    it("removing the primary promotes the next one rather than refusing", () => {
      const onChange = setup(["Sales", "Marketing"]);
      fireEvent.click(screen.getByLabelText("Remove Sales; Marketing becomes the primary project"));
      expect(onChange).toHaveBeenCalledWith(["Marketing"]);
    });

    it("offers no promote control on the primary itself — it is already primary", () => {
      setup(["Sales", "Marketing"]);
      expect(screen.queryByLabelText("Make Sales the primary project")).toBeNull();
    });
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

  // Inside a project's own file list, "Primary <that project>" on every row restates the heading
  // above them. What it must NOT do is drop anything the user could not otherwise see or reach.
  describe("hidePrimary", () => {
    it("omits the primary pill when it names the project being viewed", () => {
      render(
        <ProjectPicker value={["Sales"]} onChange={vi.fn()} suggestions={[]} hidePrimary="sales" />,
      );
      expect(screen.queryByText(/Primary/)).toBeNull();
      // The control to add another project stays: it is the only way to link one from here.
      expect(screen.getByLabelText("Add a project")).toBeTruthy();
    });

    it("still shows the other projects, which are the part you cannot infer", () => {
      render(
        <ProjectPicker
          value={["Sales", "Marketing"]}
          onChange={vi.fn()}
          suggestions={[]}
          hidePrimary="Sales"
        />,
      );
      expect(screen.queryByText(/Primary/)).toBeNull();
      expect(screen.getByLabelText("Unlink from Marketing")).toBeTruthy();
    });

    // The document is homed elsewhere and merely LINKED here. Its primary names a different
    // project, so hiding it would hide the answer to "where does this actually live?".
    it("keeps a primary that names a different project", () => {
      render(
        <ProjectPicker
          value={["Ops", "Sales"]}
          onChange={vi.fn()}
          suggestions={[]}
          hidePrimary="Sales"
        />,
      );
      expect(screen.getByText(/Primary/)).toBeTruthy();
      expect(screen.getByText("Ops")).toBeTruthy();
      // And this project's own pill keeps its remove control — the way to unlink from here.
      expect(screen.getByLabelText("Unlink from Sales")).toBeTruthy();
    });
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
