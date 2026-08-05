// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  documentLocation,
  provenanceParts,
  sourceGroup,
  sourceLabel,
  sourceSummary,
} from "./sourceLabel";

describe("sourceLabel — telling two otherwise-identical rows apart", () => {
  it("names the Google account a file lives in", () => {
    // The case that started this: the SAME file reachable from two connected accounts produced two
    // rows that rendered byte-identically. The account is the whole difference.
    expect(sourceLabel({ source_id: "gdrive:me@example.com:1AbC" })).toBe(
      "Google Drive · me@example.com",
    );
    expect(sourceLabel({ source_id: "gdrive:work@example.com:1AbC" })).toBe(
      "Google Drive · work@example.com",
    );
  });

  it("tests the account-independent namespaces BEFORE the email arm", () => {
    // `swm:` and `sd:` carry no owning account, so a naive split on ':' would report the literal
    // "swm" as an email address. `drive::account_of` orders these checks the same way.
    expect(sourceLabel({ source_id: "gdrive:swm:root1:file1" })).toBe(
      "Google Drive · shared with you",
    );
    expect(sourceLabel({ source_id: "gdrive:sd:drive1:file1" })).toBe(
      "Google Drive · a shared drive",
    );
  });

  it("covers the other connectors", () => {
    expect(sourceLabel({ source_id: "onedrive:me@example.com:01XYZ" })).toBe(
      "OneDrive · me@example.com",
    );
    expect(sourceLabel({ source_id: "local:folderkey:12345" })).toBe("This device");
  });

  it("says nothing for a vault document, where source_type already does", () => {
    expect(sourceLabel({ source_id: null })).toBeNull();
    expect(sourceLabel({ source_id: "" })).toBeNull();
    // An unrecognised prefix is not guessed at — silence beats a wrong provenance claim on a
    // screen that invites deletion.
    expect(sourceLabel({ source_id: "future-connector:x:y" })).toBeNull();
  });
});

describe("provenanceParts", () => {
  it("prefers the containing folder, which is often the only difference", () => {
    expect(
      provenanceParts({
        source_id: "gdrive:me@example.com:1AbC",
        source_parent_folder_name: "Invoices 2026",
        source_path: "/some/very/long/path/Invoices 2026/x.pdf",
      }),
    ).toEqual(["Google Drive · me@example.com", "Invoices 2026"]);
  });

  it("falls back to the path when there is no folder name", () => {
    expect(
      provenanceParts({
        source_id: "local:k:1",
        source_parent_folder_name: null,
        source_path: "/home/bobby/notes/x.md",
      }),
    ).toEqual(["This device", "/home/bobby/notes/x.md"]);
  });

  it("is empty when there is nothing to add", () => {
    expect(
      provenanceParts({ source_id: null, source_parent_folder_name: null, source_path: null }),
    ).toEqual([]);
  });
});

describe("documentLocation — the full path, for both ingest routes", () => {
  it("reads the local path for a stored document", () => {
    expect(
      documentLocation({
        source_type: "vault",
        source_path: "C:/Users/bobby/Docs/board.pdf",
        external_ref: null,
      }),
    ).toBe("C:/Users/bobby/Docs/board.pdf");
  });

  it("reads external_ref for an indexed one, which provenanceParts can never reach", () => {
    // The whole defect: an index-only row's `source_path` is structurally null, so the table's
    // path line rendered nothing — for a Drive file AND for a file in a tracked folder on this
    // machine, whose absolute path was on the row the whole time under the other column name.
    const drive = {
      source_type: "index_only" as const,
      source_path: null,
      external_ref: "https://drive.google.com/file/d/1AbC/view",
    };
    expect(documentLocation(drive)).toBe("https://drive.google.com/file/d/1AbC/view");
    expect(
      documentLocation({
        source_type: "index_only",
        source_path: null,
        external_ref: "/home/bobby/Tracked/report.docx",
      }),
    ).toBe("/home/bobby/Tracked/report.docx");
    // The fallback that made this necessary: provenanceParts stops at source_path.
    expect(
      provenanceParts({ source_id: null, source_parent_folder_name: null, source_path: null }),
    ).toEqual([]);
  });

  it("is null when neither column has anything, rather than an empty line", () => {
    expect(
      documentLocation({ source_type: "chat", source_path: null, external_ref: null }),
    ).toBeNull();
  });
});

describe("sourceGroup / sourceSummary — the axis the Source column sorts on", () => {
  it("puts a tracked local folder on THIS DEVICE, not with the clouds", () => {
    // `source_type` is "index_only" for these too, so the type cannot answer the question — the
    // source_id namespace does.
    expect(sourceGroup({ source_id: "local:k:1", source_state: "ok" })).toBe("device");
    expect(sourceGroup({ source_id: "gdrive:me@example.com:1", source_state: "ok" })).toBe("drive");
    expect(sourceGroup({ source_id: "onedrive:me@example.com:1", source_state: "ok" })).toBe(
      "onedrive",
    );
    expect(sourceGroup({ source_id: null, source_state: "ok" })).toBe("vault");
  });

  it("lets reachability outrank origin, and keeps the two kinds of trouble apart", () => {
    // The backend keeps them apart on purpose: an expired token means "ask again later", a missing
    // source means "it is gone". Collapsing them would report an outage as a deletion.
    expect(
      sourceGroup({ source_id: "gdrive:me@example.com:1", source_state: "source_missing" }),
    ).toBe("missing");
    expect(sourceGroup({ source_id: "gdrive:me@example.com:1", source_state: "unreachable" })).toBe(
      "unreachable",
    );
  });

  it("does not file an unknown namespace as held here", () => {
    // A pointer PM can't decode is still a pointer to something outside the vault.
    expect(sourceGroup({ source_id: "future-connector:x:y", source_state: "ok" })).toBe("drive");
  });

  it("says what is wrong, so a column you sorted by explains its own order", () => {
    expect(sourceSummary({ source_id: null, source_state: "ok" })).toBe("In your vault");
    expect(sourceSummary({ source_id: "local:k:1", source_state: "ok" })).toBe("This device");
    expect(
      sourceSummary({ source_id: "gdrive:me@example.com:1", source_state: "source_missing" }),
    ).toBe("Google Drive · me@example.com · not there any more");
    expect(sourceSummary({ source_id: "local:k:1", source_state: "unreachable" })).toBe(
      "This device · can’t reach it",
    );
  });
});
