// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  documentBreadcrumb,
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

describe("documentBreadcrumb — where a file sits, as folders (#736)", () => {
  /** Only the fields the breadcrumb reads; every caller passes a whole `Document`. */
  const at = (
    source_id: string | null,
    source_folder_path: string[] | null,
    source_parent_folder_name: string | null = null,
  ) => ({ source_id, source_folder_path, source_parent_folder_name });

  it("reads a My Drive trail straight through — Drive's own root is already named in it", () => {
    // The shape Bobby asked for. "My Drive" is a real folder Drive reports a name for, so the walk
    // reaches it naturally and PM must NOT prepend a second corpus label on top of it.
    expect(documentBreadcrumb(at("gdrive:a@x.com:F1", ["My Drive", "Projects", "PM"]))).toEqual([
      "My Drive",
      "Projects",
      "PM",
    ]);
  });

  it("names the collection for a file shared with you, whose trail has no root", () => {
    // The climb stops at the share boundary — the folders above it belong to someone else and are
    // invisible to this account — so the corpus name is the one crumb PM supplies. That is a fact
    // about how the file reached you, not a guess about where it sits.
    expect(documentBreadcrumb(at("gdrive:swm:R9:F1", ["crisis", "study guide"]))).toEqual([
      "Shared with you",
      "crisis",
      "study guide",
    ]);
  });

  it("says whose folders a tracked-folder trail is, and still says it at the root", () => {
    expect(documentBreadcrumb(at("local:k1:f2", ["notes", "2026"]))).toEqual([
      "This device",
      "notes",
      "2026",
    ]);
    // An empty trail is a real answer — the file sits directly in the folder you picked — and must
    // not collapse to "nothing to show".
    expect(documentBreadcrumb(at("local:k1:f2", []))).toEqual(["This device"]);
  });

  it("falls back to the one folder PM has always known, rather than to nothing", () => {
    // Two populations depend on this: OneDrive, whose ancestry PM has not verified a field for, and
    // every item indexed before the trail column existed. A one-crumb breadcrumb is not a degraded
    // trail — it is everything PM holds about that item, and it improves on its next sync.
    expect(documentBreadcrumb(at("gdrive:a@x.com:F1", null, "documentation"))).toEqual([
      "documentation",
    ]);
    expect(documentBreadcrumb(at("onedrive:a@x.com:01I", null, "Invoices"))).toEqual([
      "OneDrive",
      "Invoices",
    ]);
    // But it never invents a corpus label with no folder to hang it on: a bare "OneDrive" says
    // nothing the Source column doesn't already say.
    expect(documentBreadcrumb(at("onedrive:a@x.com:01I", null, null))).toEqual([]);
  });

  it("distinguishes an unresolved trail from a resolved empty one", () => {
    // NULL is "PM hasn't looked", [] is "it sits at the top" — the difference the sync path spends
    // (or saves) requests on, and it must survive all the way to the render.
    expect(documentBreadcrumb(at("gdrive:swm:R9:F1", []))).toEqual(["Shared with you"]);
    expect(documentBreadcrumb(at("gdrive:swm:R9:F1", null))).toEqual(["Shared with you"]);
    expect(documentBreadcrumb(at("gdrive:a@x.com:F1", null))).toEqual([]);
  });

  it("has nothing to say about a document no connector found", () => {
    expect(documentBreadcrumb(at(null, null))).toEqual([]);
    expect(documentBreadcrumb(at(null, ["ignored"]))).toEqual([]);
  });
});
