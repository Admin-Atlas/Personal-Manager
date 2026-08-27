// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The model-licences gate's own rules.
//
// The property most worth pinning is the one a naive gate would miss: the catalogue carries a COPY
// of each licence, resolved from the ledger at generation time, and a copy can drift. A gate that
// only checked "every entry has a licence id" would wave through a catalogue whose summary text no
// longer matches the terms a human approved — and that summary is exactly what a user is shown
// before their machine fetches the weights.
//
// Importing the module does not run the gate — entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  contentHash,
  rustSchemaVersion,
  scan,
  CATALOGUE_FILE,
  LEDGER_FILE,
} from "./check-model-licences.mjs";

const APACHE = {
  name: "Apache License 2.0",
  url: "https://www.apache.org/licenses/LICENSE-2.0",
  open: true,
  summary: "A permissive open-source licence.",
};
const GEMMA = {
  name: "Gemma Terms of Use",
  url: "https://ai.google.dev/gemma/terms",
  open: false,
  summary: "Google's own terms, not an open-source licence.",
};

const licenceBlock = (id, term) => ({
  id,
  name: term.name,
  url: term.url,
  open: term.open,
  summary: term.summary,
});

/** One catalogue entry; only the fields this gate looks at need to be real.
 *
 *  The quant row carries a real-shaped `ollama` tag because the gate now derives the expected value
 *  from the row itself — a fixture with no tag is a catalogue whose Download button is dead, which
 *  is precisely the shipped state the tag rules exist to catch. */
const entry = (repo, id, term, over = {}) => ({
  repo,
  licence: licenceBlock(id, term),
  quants: [{ quant: "Q4_K_M", file_gb: 1.5, sharded: false, ollama: `hf.co/${repo}:Q4_K_M` }],
  ...over,
});

/**
 * A throwaway tree. `content_hash` is computed from the entries as given, so a test that wants the
 * hash check to fire has to break it deliberately rather than by accident.
 */
function fixture({ entries, terms, models, schemaVersion = 2, hash }) {
  const root = mkdtempSync(join(tmpdir(), "pm-model-licences-"));
  mkdirSync(join(root, "src-tauri", "src"), { recursive: true });
  writeFileSync(
    join(root, CATALOGUE_FILE),
    JSON.stringify({
      schema_version: schemaVersion,
      content_hash: hash ?? contentHash(entries),
      entries,
    }),
  );
  writeFileSync(join(root, LEDGER_FILE), JSON.stringify({ version: 1, terms, models }));
  writeFileSync(
    join(root, "src-tauri", "src", "local_catalog.rs"),
    'assert_eq!(\n    cat.schema_version, 2,\n    "catalog schema version"\n);',
  );
  return root;
}

/** Enough well-formed rows to clear the floor, so a test's own case is what fails. */
function filler(count) {
  const entries = [];
  const models = {};
  for (let i = 0; i < count; i++) {
    entries.push(entry(`org/filler${i}`, "apache-2.0", APACHE));
    models[`org/filler${i}`] = { role: "chat", licence: "apache-2.0" };
  }
  return { entries, models };
}

describe("rustSchemaVersion", () => {
  it("reads the pinned version out of the parse-guard assertion", () => {
    expect(rustSchemaVersion('assert_eq!(\n  cat.schema_version, 7,\n  "msg"\n);')).toBe(7);
  });

  it("throws rather than guessing when the assertion has moved", () => {
    expect(() => rustSchemaVersion("assert!(cat.schema_version >= 1);")).toThrow(
      /could not read the pinned schema_version/,
    );
  });
});

describe("scan", () => {
  it("passes on the real tree and sees both open and restricted models", () => {
    const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
    const { problems, count, restricted } = scan(root);
    expect(problems).toEqual([]);
    expect(count).toBeGreaterThan(10);
    // If this ever reads zero, the terms dialog has quietly stopped being reachable by any model.
    expect(restricted).toBeGreaterThan(0);
  });

  it("passes a catalogue that is a strict subset of the ledger", () => {
    // Normal: the generator drops a seed it cannot resolve (no curated quant, unreadable MoE
    // header), and that seed keeps its licence row so it cannot re-enter unreviewed.
    const { entries, models } = filler(12);
    const root = fixture({
      entries,
      terms: { "apache-2.0": APACHE },
      models: { ...models, "org/not-yet": { role: "chat", licence: "apache-2.0" } },
    });
    expect(scan(root).problems).toEqual([]);
  });

  it("fails a ledger row whose licence nobody has decided", () => {
    const { entries, models } = filler(12);
    const root = fixture({
      entries,
      terms: { "apache-2.0": APACHE },
      models: { ...models, "org/mystery": { role: "chat", licence: null } },
    });
    expect(scan(root).problems.join(" ")).toMatch(/org\/mystery has no reviewed licence/);
  });

  it("fails a licence id that names no row in the terms table", () => {
    const { entries, models } = filler(12);
    models["org/filler0"].licence = "invented";
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models });
    expect(scan(root).problems.join(" ")).toMatch(
      /org\/filler0 is recorded as `invented`, which is not a row in `terms`/,
    );
  });

  it("fails a restricted licence with no summary — an empty dialog", () => {
    const { entries, models } = filler(12);
    entries.push(entry("org/gemma", "gemma", { ...GEMMA, summary: "  " }));
    models["org/gemma"] = { role: "chat", licence: "gemma" };
    const root = fixture({
      entries,
      terms: { "apache-2.0": APACHE, gemma: { ...GEMMA, summary: "  " } },
      models,
    });
    expect(scan(root).problems.join(" ")).toMatch(/terms.gemma has no summary/);
  });

  it("fails a terms row no model references", () => {
    const { entries, models } = filler(12);
    const root = fixture({ entries, terms: { "apache-2.0": APACHE, gemma: GEMMA }, models });
    expect(scan(root).problems.join(" ")).toMatch(/terms.gemma is not used by any model/);
  });

  it("catches the catalogue's copy drifting from the ledger it came from", () => {
    // The case the whole gate exists for: someone corrects the terms text in the ledger and never
    // regenerates, so the app goes on showing the paragraph nobody approved.
    const { entries, models } = filler(12);
    const root = fixture({
      entries,
      terms: { "apache-2.0": { ...APACHE, summary: "Corrected wording that never shipped." } },
      models,
    });
    expect(scan(root).problems.join(" ")).toMatch(/licence `summary` has drifted/);
  });

  it("catches a catalogue entry claiming a different licence from its ledger row", () => {
    const { entries, models } = filler(12);
    entries[0] = entry("org/filler0", "gemma", GEMMA);
    const root = fixture({ entries, terms: { "apache-2.0": APACHE, gemma: GEMMA }, models });
    const joined = scan(root).problems.join(" ");
    expect(joined).toMatch(/says `gemma` but .* records `apache-2\.0`/);
  });

  it("catches an unknown field on a licence block before it panics the Rust side", () => {
    const { entries, models } = filler(12);
    entries[0].licence.notes = "hand-added";
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models });
    expect(scan(root).problems.join(" ")).toMatch(/unknown field\(s\) notes/);
  });

  it("catches a hand-edited catalogue through its content hash", () => {
    const { entries, models } = filler(12);
    const root = fixture({
      entries,
      terms: { "apache-2.0": APACHE },
      models,
      hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    });
    expect(scan(root).problems.join(" ")).toMatch(/was edited by hand/);
  });

  it("catches a schema bump on one side only", () => {
    const { entries, models } = filler(12);
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models, schemaVersion: 3 });
    expect(scan(root).problems.join(" ")).toMatch(/schema_version 3 but .* pins 2/);
  });

  it("fails a truncated catalogue rather than reporting a clean scan of nothing", () => {
    const { entries, models } = filler(2);
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models });
    expect(scan(root).problems.join(" ")).toMatch(/catalogue is truncated, or this parser/);
  });

  // The Ollama pull tags. The shipped bug these guard is that every entry carried a null one for
  // three releases and no gate looked, so the Download button could not render for any model.
  const tagged = (over) => {
    const { entries, models } = filler(12);
    Object.assign(entries[0].quants[0], over);
    return fixture({ entries, terms: { "apache-2.0": APACHE }, models });
  };

  it("catches a tag that names another model", () => {
    // Derived from the row, not pattern-matched: a /^hf\.co\// test would wave this through, and
    // pointing the Download button at a different model is the worst version of this failure.
    expect(scan(tagged({ ollama: "hf.co/someone/else:Q4_K_M" })).problems.join(" ")).toMatch(
      /tag is "hf\.co\/someone\/else:Q4_K_M", expected/,
    );
  });

  it("catches a tag on a sharded quant, which Ollama's registry refuses", () => {
    expect(scan(tagged({ sharded: true })).problems.join(" ")).toMatch(
      /sharded GGUF carries a pull tag/,
    );
  });

  it("catches a quant row the generator wrote no tag field onto at all", () => {
    const { entries, models } = filler(12);
    delete entries[0].quants[0].ollama;
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models });
    expect(scan(root).problems.join(" ")).toMatch(/the generator did not write one/);
  });

  it("catches the shipped state: no entry downloadable, and the retired install key", () => {
    const { entries, models } = filler(12);
    for (const e of entries) {
      e.quants[0].ollama = null;
      e.install = { ollama: null };
    }
    const root = fixture({ entries, terms: { "apache-2.0": APACHE }, models });
    const problems = scan(root).problems.join(" ");
    expect(problems).toMatch(/only 0 of 12 entries carry an Ollama pull tag/);
    expect(problems).toMatch(/still carries the retired per-entry/);
  });
});
