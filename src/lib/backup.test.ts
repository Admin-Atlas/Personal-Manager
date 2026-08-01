// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  archiveStampIso,
  describeFailures,
  describeForgetConsequences,
  isOpaquePhase,
  visibleRetentionNotes,
} from "./backup";

describe("isOpaquePhase — shimmer vs percent bar (F-45)", () => {
  it("treats upload and download as opaque (they have no honest byte fraction)", () => {
    expect(isOpaquePhase("upload")).toBe(true);
    expect(isOpaquePhase("download")).toBe(true);
  });

  it("treats snapshot and validate as opaque — they emit only 0 and 1", () => {
    // This pair used to be pinned `false` under a comment claiming snapshot was "byte-metered via
    // ProgressReader". It never was: ProgressReader appears only in pack.rs (:127/:140) and
    // restore.rs (:134). `VACUUM INTO` is one opaque SQLite call — `begin_backup_run` opens the run
    // on Snapshot and the next emission is fraction 1.0 (commands/backups.rs:122-129, :765-772) —
    // and Validate is the same shape (restore.rs:201 → 0.0, :238 → 1.0). So both rendered as a bar
    // frozen at 0% for the whole phase, which on a large store is the longest stretch of a backup.
    expect(isOpaquePhase("snapshot")).toBe(true);
    expect(isOpaquePhase("validate")).toBe(true);
  });

  it("leaves the genuinely metered phases as determinate", () => {
    // pack and restore read through ProgressReader, so their fractions are real byte counts.
    expect(isOpaquePhase("pack")).toBe(false);
    expect(isOpaquePhase("restore")).toBe(false);
  });

  it("is safe on a null phase (no op in flight)", () => {
    expect(isOpaquePhase(null)).toBe(false);
  });
});

describe("describeFailures — partial-failure banner copy (F-22)", () => {
  it("returns null on a clean run so the banner stays hidden", () => {
    expect(describeFailures([])).toBeNull();
  });

  it("uses the singular noun for one failed destination", () => {
    const msg = describeFailures(["Google Drive: 401 unauthorized"]);
    expect(msg).toContain("1 destination failed");
    expect(msg).not.toContain("destinations failed");
    expect(msg).toContain("Google Drive: 401 unauthorized");
  });

  it("uses the plural noun and joins multiple failures", () => {
    const msg = describeFailures(["Proton Drive: timed out", "Google Drive: quota"]);
    expect(msg).toContain("2 destinations failed");
    expect(msg).toContain("Proton Drive: timed out; Google Drive: quota");
  });

  it("reassures that the successful destinations still got the archive", () => {
    // The banner is non-blocking: at least one destination succeeded (that's the only time the
    // backend populates failed_destinations), so the copy must not read as a total failure.
    expect(describeFailures(["X: nope"])).toContain("did reach the destinations that succeeded");
  });
});

describe("describeForgetConsequences — the SILENT half of forgetting the passphrase", () => {
  it("says nothing when there is no schedule to lose", () => {
    // A false alarm is its own defect: someone who never turned automatic backups on must not be
    // warned that this turns them off.
    expect(describeForgetConsequences("off")).toBeNull();
  });

  it("names the user's own cadence, and that it becomes Off", () => {
    for (const [freq, label] of [
      ["daily", "Daily"],
      ["weekly", "Weekly"],
      ["monthly", "Monthly"],
    ] as const) {
      const msg = describeForgetConsequences(freq);
      expect(msg).toContain(label);
      expect(msg).toContain("switches them to Off");
    }
  });

  it("says what SURVIVES, so the warning isn't read as 'this deletes my backups'", () => {
    // `forget_backup_passphrase` touches the cadence and the keychain and nothing else — the
    // destinations, the retention count and every archive are untouched. The dialog that carries
    // this sentence also carries the unreadable-backups claim, so under-stating what survives
    // would make it read as a delete.
    const msg = describeForgetConsequences("weekly") ?? "";
    expect(msg).toContain("untouched");
    expect(msg).toContain("how many backups to keep");
  });
});

describe("archiveStampIso — reading the date the filename was hiding", () => {
  it("expands the backend's compact stamp into something Date can parse", () => {
    // The producer is `pm-backup-{vaultid}-{YYYYMMDDTHHMMSSZ}.pmbackup` (backup/naming.rs).
    const iso = archiveStampIso("pm-backup-abc123-20260801T161659Z.pmbackup");
    expect(iso).toBe("2026-08-01T16:16:59Z");
    // The whole reason for expanding: JS parses only extended ISO.
    expect(Number.isNaN(new Date(iso as string).getTime())).toBe(false);
    expect(Number.isNaN(new Date("20260801T161659Z").getTime())).toBe(true);
  });

  it("returns null for a name that isn't ours, so the row still renders", () => {
    // The listing filters on extension alone, so a foreign .pmbackup can sit in the same folder.
    expect(archiveStampIso("someone-elses.pmbackup")).toBeNull();
    expect(archiveStampIso("pm-backup-abc123-notastamp.pmbackup")).toBeNull();
    expect(archiveStampIso("pm-backup-abc123-20260801T161659Z.txt")).toBeNull();
  });
});

describe("visibleRetentionNotes — heals on evidence, never on absence of it", () => {
  const overLimit = {
    kind: "gdrive",
    message: "Google Drive: 3 could not be trimmed",
    over_limit: true,
  };
  const trimFailed = {
    kind: "gdrive",
    message: "Google Drive: trimming old backups failed",
    over_limit: false,
  };

  it("drops a count note once a fresh listing shows the destination back under its limit", () => {
    // This is the reported case: the user deletes the extra archives in Drive, and the note goes
    // on the next visit with no new IPC and no polling.
    expect(visibleRetentionNotes([overLimit], () => false)).toEqual([]);
  });

  it("keeps a count note while the destination is still over the limit", () => {
    expect(visibleRetentionNotes([overLimit], () => true)).toEqual([overLimit]);
  });

  it("keeps a count note when the listing is UNKNOWN — the safety property", () => {
    // null covers: listing still loading, the request threw, and the write scope is missing. In
    // all three "not over the limit" is an absence of evidence, and suppressing there would hide
    // a true warning exactly when PM can least see the destination.
    expect(visibleRetentionNotes([overLimit], () => null)).toEqual([overLimit]);
  });

  it("never suppresses a trim FAILURE by a count", () => {
    // A listing that succeeds says nothing about whether the trim would now work.
    expect(visibleRetentionNotes([trimFailed], () => false)).toEqual([trimFailed]);
  });

  it("suppresses per destination, not globally", () => {
    const protonNote = { ...overLimit, kind: "proton" };
    expect(
      visibleRetentionNotes([overLimit, protonNote], (k) => (k === "gdrive" ? false : true)),
    ).toEqual([protonNote]);
  });
});
