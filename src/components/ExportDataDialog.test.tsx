// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// What the export chooser (#712) must never get wrong, all of it invisible in the markup:
//
//   - the two axes route to three DIFFERENT backends, and picking the wrong one either writes an
//     unreadable archive or writes a readable one where the user asked for a locked file;
//   - the plain/everything cell must not claim to be readable — PM's store stays SQLCipher-encrypted
//     inside that zip, and DECISIONS.md refuses an unencrypted copy. A dialog implying otherwise
//     would be a security claim PM does not make;
//   - the fourth cell does not exist, and must SAY so rather than silently doing something else.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const exportAllData = vi.fn();
const exportPlaintextMarkdown = vi.fn();
const createLocalBackup = vi.fn();
const saveFileDialog = vi.fn();

vi.mock("../lib/ipc", () => ({
  exportAllData: (dest: string) => exportAllData(dest),
  exportPlaintextMarkdown: () => exportPlaintextMarkdown(),
  createLocalBackup: (dest: string, pass: string) => createLocalBackup(dest, pass),
  // The strength meter's own call.
  scorePassphrase: vi.fn(async () => ({ score: 4, acceptable: true, feedback: [] })),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (opts: unknown) => saveFileDialog(opts) }));

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

import { ExportDataDialog } from "./ExportDataDialog";

const PASS = "correct horse battery staple";
const disabled = (el: HTMLElement) => (el as HTMLButtonElement).disabled;

function open() {
  render(<ExportDataDialog open onClose={() => {}} />);
}

const pick = (name: RegExp) => screen.getByRole("button", { name });

beforeEach(() => {
  vi.clearAllMocks();
  saveFileDialog.mockResolvedValue("D:\\out.zip");
  exportAllData.mockResolvedValue(undefined);
  createLocalBackup.mockResolvedValue(undefined);
  exportPlaintextMarkdown.mockResolvedValue({ count: 12, dest: "D:\\md" });
});

afterEach(cleanup);

describe("the two axes", () => {
  it("defaults to everything + plain, and writes the zip", async () => {
    open();
    fireEvent.click(pick(/^export…$/i));
    await waitFor(() => expect(exportAllData).toHaveBeenCalledWith("D:\\out.zip"));
    expect(createLocalBackup).not.toHaveBeenCalled();
    expect(exportPlaintextMarkdown).not.toHaveBeenCalled();
  });

  it("never claims the whole archive is readable", async () => {
    // The store stays SQLCipher-encrypted inside that zip, by decision. A dialog that said
    // "readable" without qualification would be a security claim PM does not make.
    open();
    expect(screen.getByText(/store stays encrypted inside the archive/i)).toBeTruthy();
  });

  it("sends the documents-only plain export to the backend's own folder picker", async () => {
    // It writes DECRYPTED content, so the destination must not be a path a compromised webview
    // could fabricate — the backend picks the folder itself (L-5). A save dialog here would be the
    // bug, not a shortcut.
    open();
    fireEvent.click(pick(/just my documents/i));
    fireEvent.click(pick(/^export…$/i));
    await waitFor(() => expect(exportPlaintextMarkdown).toHaveBeenCalledTimes(1));
    expect(saveFileDialog).not.toHaveBeenCalled();
    expect(exportAllData).not.toHaveBeenCalled();
    expect(await screen.findByText(/Exported 12 Markdown files/)).toBeTruthy();
  });

  it("writes a .pmbackup for the encrypted cell, and says it is the same thing as a backup", async () => {
    open();
    fireEvent.click(pick(/^encrypted$/i));
    expect(screen.getByText(/exactly what PM's backup writes/i)).toBeTruthy();

    fireEvent.change(screen.getByPlaceholderText("Passphrase for this archive"), {
      target: { value: PASS },
    });
    fireEvent.change(screen.getByPlaceholderText("Confirm passphrase"), {
      target: { value: PASS },
    });
    await waitFor(() => expect(disabled(pick(/^export…$/i))).toBe(false));
    fireEvent.click(pick(/^export…$/i));
    await waitFor(() => expect(createLocalBackup).toHaveBeenCalledWith("D:\\out.zip", PASS));
    expect(exportAllData).not.toHaveBeenCalled();
  });

  it("will not write an encrypted archive until the passphrase is confirmed", async () => {
    // An archive locked with a passphrase the user mistyped is a file nobody can ever open.
    open();
    fireEvent.click(pick(/^encrypted$/i));
    expect(disabled(pick(/^export…$/i))).toBe(true);
    fireEvent.change(screen.getByPlaceholderText("Passphrase for this archive"), {
      target: { value: PASS },
    });
    fireEvent.change(screen.getByPlaceholderText("Confirm passphrase"), {
      target: { value: "something else" },
    });
    expect(disabled(pick(/^export…$/i))).toBe(true);
  });
});

describe("the cell that does not exist", () => {
  it("refuses documents-only + encrypted in words, not by quietly doing something else", async () => {
    // Restoring a .pmbackup hard-requires pm.sqlite and vault-meta.json, so a documents-only one
    // would be a file the user could never restore. Falling back to either neighbouring cell would
    // hand them a different archive from the one they asked for.
    open();
    fireEvent.click(pick(/just my documents/i));
    fireEvent.click(pick(/^encrypted$/i));

    expect(screen.getByRole("alert").textContent).toMatch(/can.t make an encrypted archive/i);
    expect(disabled(pick(/^export…$/i))).toBe(true);

    fireEvent.click(pick(/^export…$/i));
    expect(createLocalBackup).not.toHaveBeenCalled();
    expect(exportPlaintextMarkdown).not.toHaveBeenCalled();
    expect(exportAllData).not.toHaveBeenCalled();
  });

  it("clears the refusal as soon as the choice becomes possible again", async () => {
    open();
    fireEvent.click(pick(/just my documents/i));
    fireEvent.click(pick(/^encrypted$/i));
    fireEvent.click(pick(/^everything$/i));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
