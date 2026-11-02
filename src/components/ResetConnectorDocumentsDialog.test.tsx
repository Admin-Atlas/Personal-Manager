// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The connector reset (#1019) throws away the filing of what can be a whole library, so what the
// dialog promises — and that it can't be confirmed without the tick — is worth pinning.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ResetConnectorDocumentsDialog,
  connectorDocumentCount,
} from "./ResetConnectorDocumentsDialog";
import type { Document } from "../lib/types";

const reset = vi.fn(() => Promise.resolve(2));
vi.mock("../lib/ipc", () => ({ resetConnectorDocuments: () => reset() }));

// The dialog's <Button>s reach for `useTheme`, and the real ThemeProvider pulls in IPC.
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

const doc = (source_type: Document["source_type"]) => ({ source_type }) as Document;
const docs = [doc("index_only"), doc("index_only"), doc("vault"), doc("chat")];

afterEach(cleanup);

describe("ResetConnectorDocumentsDialog (#1019)", () => {
  it("counts only the connector files", () => {
    expect(connectorDocumentCount(docs)).toBe(2);
  });

  it("says the source files and the vault are untouched, and can't confirm unticked", () => {
    render(<ResetConnectorDocumentsDialog docs={docs} onClose={() => {}} onReset={() => {}} />);
    expect(screen.getByText(/removes the 2 files it indexed/)).toBeTruthy();
    expect(screen.getByText(/aren’t touched/)).toBeTruthy();
    expect(screen.getByText(/your chats, aren’t affected/)).toBeTruthy();
    const confirm = screen.getByRole("button", { name: "Reset and fetch again" });
    expect((confirm as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(confirm);
    expect(reset).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("checkbox"));
    expect((confirm as HTMLButtonElement).disabled).toBe(false);
  });
});
