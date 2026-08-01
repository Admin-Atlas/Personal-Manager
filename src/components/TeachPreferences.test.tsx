// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The preference dialog's five fields, and what a parse failure does.
//
// Every one of the five wore a visual `<label>` that named nothing — no `htmlFor`, no wrapped
// control — so a screen reader announced the scope picker as "combobox" and the preference body as
// its placeholder ("keep replies short and to the point"), which stops being the name the moment
// you type. The queries below are the accessible-NAME computation, which is the only thing that
// settles whether the fix landed; the markup looks identical either way.
//
// The parse failure is the same defect one layer up: it rendered as a silent red paragraph. Press
// "Fill in", have it fail, and a blind user got no announcement and no changed fields — the app
// simply appeared to do nothing.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listPreferences = vi.fn();
const parsePreferenceStatement = vi.fn();

vi.mock("../lib/ipc", () => ({
  addPreference: () => Promise.resolve(),
  confirmPreference: () => Promise.resolve(),
  deletePreference: () => Promise.resolve(),
  listPreferences: () => listPreferences(),
  parsePreferenceStatement: (text: string) => parsePreferenceStatement(text),
  updatePreference: () => Promise.resolve(),
}));

vi.mock("../lib/capabilities", () => ({
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
  isDevBuild: false,
}));

// The same stub the other component tests use: <Button>/<Input> reach for `useTheme`, and the real
// ThemeProvider pulls in IPC. `useDepth` reads it too, so this covers both.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { TeachPreferences } from "./TeachPreferences";

afterEach(cleanup);

beforeEach(() => {
  vi.clearAllMocks();
  listPreferences.mockResolvedValue([]);
  parsePreferenceStatement.mockResolvedValue({
    scope: "global",
    entity_id: null,
    condition: null,
    value: "file invoices under Finances",
  });
});

/** Render the section and open the add dialog, which is where all five fields live. */
async function openDialog() {
  render(<TeachPreferences projects={[]} />);
  fireEvent.click(await screen.findByRole("button", { name: "Add" }));
  await screen.findByRole("dialog");
}

describe("the preference dialog's fields", () => {
  it("names every field by its visible label", async () => {
    await openDialog();

    expect(screen.getByLabelText("In your own words")).toBeTruthy();
    expect(screen.getByLabelText("Applies")).toBeTruthy();
    expect(screen.getByLabelText("Preference")).toBeTruthy();
  });

  it("does not fall back to the placeholder for a name", async () => {
    await openDialog();

    expect(screen.queryByLabelText("keep replies short and to the point")).toBeNull();
    expect(screen.queryByLabelText("e.g. file invoices under Finances")).toBeNull();
  });

  it("reveals the project picker named, once the scope asks for one", async () => {
    // "Project" only exists in the SCOPE_PROJECT branch, so it is the field most likely to be
    // missed by a conversion that only reads the default render.
    await openDialog();
    fireEvent.change(screen.getByLabelText("Applies"), { target: { value: "project" } });

    expect(await screen.findByLabelText("Project")).toBeTruthy();
  });
});

describe("a failed parse", () => {
  it("is announced, and marks the field it came from", async () => {
    parsePreferenceStatement.mockRejectedValue(new Error("could not read that"));
    await openDialog();

    const input = screen.getByLabelText("In your own words");
    fireEvent.change(input, { target: { value: "file invoices under Finances" } });
    fireEvent.click(screen.getByRole("button", { name: "Fill in" }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("could not read that");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toContain(alert.id);
  });

  it("says nothing when the parse succeeds", async () => {
    await openDialog();

    fireEvent.change(screen.getByLabelText("In your own words"), {
      target: { value: "file invoices under Finances" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Fill in" }));

    await waitFor(() =>
      expect(screen.getByLabelText("Preference")).toHaveProperty(
        "value",
        "file invoices under Finances",
      ),
    );
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByLabelText("In your own words").getAttribute("aria-invalid")).toBeNull();
  });
});
