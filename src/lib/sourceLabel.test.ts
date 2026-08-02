// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { provenanceParts, sourceLabel } from "./sourceLabel";

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
