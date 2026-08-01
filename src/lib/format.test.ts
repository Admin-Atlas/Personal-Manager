// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  formatBytes,
  formatDate,
  formatDateOnly,
  formatDateLocal,
  formatDateTime,
  formatGib,
  formatSyncedShort,
  formatWhen,
} from "./format";

// Dates are round-tripped through a *local* Date so the assertions hold regardless of the runner's
// timezone: `new Date(y, m, d, 12).toISOString()` names one instant, and parsing it back lands on the
// same local calendar day (noon has ~12h of slack either side of a day boundary).
function localIso(y: number, m0: number, d: number): string {
  return new Date(y, m0, d, 12, 0, 0).toISOString();
}

describe("formatDate", () => {
  it("renders a past-year date as DD-MM-YYYY", () => {
    expect(formatDate(localIso(2024, 2, 5))).toBe("05-03-2024"); // month is 0-indexed: 2 = March
  });

  it("drops the year for a date in the current year", () => {
    const y = new Date().getFullYear();
    expect(formatDate(localIso(y, 5, 9))).toBe("09-06"); // 5 = June
  });

  it("zero-pads day and month", () => {
    expect(formatDate(localIso(2023, 0, 1))).toBe("01-01-2023");
  });

  it("returns an unparseable value unchanged", () => {
    expect(formatDate("not a date")).toBe("not a date");
    expect(formatDate("")).toBe("");
  });
});

describe("formatDateOnly", () => {
  it("parses a bare YYYY-MM-DD into the local calendar day (no UTC shift)", () => {
    // F-14: formatDate('2024-03-05') reads UTC midnight and lands a day early in UTC-negative zones;
    // formatDateOnly builds from the y/m/d fields, so it is stable in every timezone.
    expect(formatDateOnly("2024-03-05")).toBe("05-03-2024");
    expect(formatDateOnly("2023-12-31")).toBe("31-12-2023");
  });

  it("uses only the written date part of a full ISO timestamp", () => {
    expect(formatDateOnly("2024-03-05T23:30:00Z")).toBe("05-03-2024");
  });

  it("drops the year in the current year", () => {
    const y = new Date().getFullYear();
    expect(formatDateOnly(`${y}-06-09`)).toBe("09-06");
  });

  it("falls back for a non-date value", () => {
    expect(formatDateOnly("not a date")).toBe("not a date");
  });
});

describe("formatDateLocal", () => {
  it("formats a local Date's own calendar fields (no ISO round-trip)", () => {
    expect(formatDateLocal(new Date(2024, 2, 5))).toBe("05-03-2024");
  });

  it("returns empty string for an invalid Date", () => {
    expect(formatDateLocal(new Date("nope"))).toBe("");
  });
});

describe("formatDateTime", () => {
  it("is the date plus a HH:MM clock", () => {
    const rendered = formatDateTime(localIso(2024, 2, 5));
    expect(rendered.startsWith("05-03-2024 ")).toBe(true);
    expect(rendered).toMatch(/\d{2}:\d{2}(\s?[AP]M)?$/i);
  });

  it("returns an unparseable value unchanged", () => {
    expect(formatDateTime("garbage")).toBe("garbage");
  });
});

describe("formatWhen", () => {
  it("returns an unparseable value unchanged", () => {
    expect(formatWhen("garbage")).toBe("garbage");
  });
});

describe("formatSyncedShort", () => {
  // This drives the Refresh button's label, so the day boundary is the whole point: a bare clock
  // time for yesterday's sync would read as today, on the very control you press to fix that.
  const now = new Date(2026, 6, 26, 15, 0, 0); // 26 July 2026, 15:00 local

  it("shows a clock time for a sync that happened today", () => {
    expect(formatSyncedShort(new Date(2026, 6, 26, 9, 30).toISOString(), now)).toMatch(
      /^\d{2}:\d{2}(\s?[AP]M)?$/i,
    );
  });

  it("shows the date once the sync is not from today", () => {
    expect(formatSyncedShort(new Date(2026, 6, 25, 23, 59).toISOString(), now)).toBe("25-07");
  });

  it("is empty for an unparseable value", () => {
    expect(formatSyncedShort("garbage", now)).toBe("");
  });
});

// The one byte formatter. Four existed — this one, StorageSettings' `formatSize`, and LocalAiSettings'
// `fmtGb`/`fmtBytes` — and `fmtBytes` was DECIMAL while everything it sat next to was binary, so the
// same model read "4.7 GB" as it downloaded and "4.3 GB" the instant it landed. The base is the whole
// point of this suite: every `*_gb` figure crossing IPC is already GiB by design (`hardware.rs`'s
// `GIB`, `local_disk.rs::bytes_to_gb`, the catalog's `file_gb`), and `fit.rs` compares those three
// against each other — so a decimal presentation contradicts the numbers underneath it.

describe("formatBytes", () => {
  it("steps in binary, not decimal", () => {
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1024 ** 2)).toBe("1 MB");
    expect(formatBytes(1024 ** 3)).toBe("1.0 GB");
    expect(formatBytes(1024 ** 4)).toBe("1.0 TB");
  });

  it("prints the byte count that used to read 4.7 GB as 4.3 GB", () => {
    // The regression net for the whole finding: this is a real Ollama pull size, and it now agrees
    // to the digit with the on-disk card and the catalog that scored the fit.
    expect(formatBytes(4_661_211_808)).toBe("4.3 GB");
    expect(formatBytes(8_988_110_656)).toBe("8.4 GB");
  });

  it("promotes instead of rounding to a full unit", () => {
    // Round FIRST, then promote. The old version rounded within the unit it had already chosen, so
    // one byte under a boundary printed "1024 KB" and "1024 GB" — units that do not exist.
    expect(formatBytes(1024 ** 2 - 1)).toBe("1 MB");
    expect(formatBytes(1024 ** 4 - 1)).toBe("1.0 TB");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("gives one decimal from GB up and whole numbers below", () => {
    expect(formatBytes(152_043_520)).toBe("145 MB");
    expect(formatBytes(98_304)).toBe("96 KB");
    expect(formatBytes(12)).toBe("12 B");
  });

  it("survives the degenerate inputs it now has to, as the pull-progress formatter", () => {
    // `fmtBytes` was called with a nullable `completed_bytes`, so the shared function inherits every
    // shape the stream can produce. `formatBytes(-5)` used to return the literal "NaN undefined".
    expect(formatBytes(null)).toBe("—");
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(NaN)).toBe("—");
    expect(formatBytes(Infinity)).toBe("—");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-5)).toBe("0 B");
  });
});

describe("formatGib", () => {
  it("is byte-identical to the old fmtGb at every value the hardware grid shows", () => {
    // The six `*_gb` call sites must not churn: this is the no-change contract.
    expect(formatGib(16)).toBe("16.0 GB");
    expect(formatGib(8.4)).toBe("8.4 GB");
    expect(formatGib(128)).toBe("128.0 GB");
    expect(formatGib(4.34)).toBe("4.3 GB");
  });

  it("changes only where the old output was wrong", () => {
    // "0.0 GB free of 16.0 GB" read as zero on a machine under memory pressure.
    expect(formatGib(0.04)).toBe("41 MB");
    expect(formatGib(0.4)).toBe("410 MB");
    expect(formatGib(1500)).toBe("1.5 TB");
  });

  it("renders an absent figure as an em dash", () => {
    expect(formatGib(null)).toBe("—");
    expect(formatGib(undefined)).toBe("—");
    expect(formatGib(0)).toBe("0 B");
  });
});
