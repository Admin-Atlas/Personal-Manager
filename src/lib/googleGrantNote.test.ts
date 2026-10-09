// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { googleGrantNote } from "./googleGrantNote";
import type { GoogleUse } from "./types";

const note = (service: "calendar" | "drive", keptFor: GoogleUse[]) =>
  googleGrantNote({ service, email: "me@example.com", keptFor });

describe("googleGrantNote", () => {
  it("says nothing when the disconnect revoked PM's access", () => {
    expect(note("calendar", [])).toBeNull();
  });

  it("names the account, the service that left, and the one that kept the access", () => {
    expect(note("calendar", ["drive"])).toBe(
      "Disconnected me@example.com from Google Calendar. PM still uses this account for Google Drive, so Google keeps PM's access to it. To remove PM's access completely, disconnect that too, or remove PM at",
    );
    expect(note("drive", ["calendar"])).toContain(
      "Disconnected me@example.com from Google Drive. PM still uses this account for Google Calendar,",
    );
  });

  // Turning backups off never revokes (#600), so the note must not promise that it removes access.
  it("never tells you that turning off backups removes PM's access", () => {
    const onlyBackup = note("drive", ["backup"]);
    expect(onlyBackup).toBe(
      "Disconnected me@example.com from Google Drive. PM still uses this account for backups, so Google keeps PM's access to it. Turning off backups keeps that access too, so PM can still tidy the backups it already made. To remove it completely, remove PM at",
    );
    expect(onlyBackup).not.toContain("disconnect that too");
    expect(note("calendar", ["drive", "backup"])).toContain(
      "for Google Drive and backups, so Google keeps PM's access to it. Turning off backups keeps that access too, so PM can still tidy the backups it already made. To remove it completely, disconnect Google Drive and remove PM at",
    );
  });

  it("joins three features into one list", () => {
    expect(note("drive", ["calendar", "drive", "backup"])).toContain(
      "for Google Calendar, Google Drive and backups, so",
    );
  });
});
