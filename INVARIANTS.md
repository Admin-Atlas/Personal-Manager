# INVARIANTS.md — contracts more than one change has to co-sign

Most rules in this repo belong to one module and are enforced where they live. This file is for
the other kind: a contract that only holds if **two or more separate pieces of work agree to it**,
where the second one is written months after the first, by someone who never saw the reasoning.
Those are the ones that break silently — nothing fails, the feature just quietly stops being true.

Read this alongside [AGENTS.md](AGENTS.md). AGENTS.md says how PM is built and lists the
non-negotiables; this file says what must stay true *across* changes. The sibling register
[SYNC-SET.md](SYNC-SET.md) does the same job for one specific question — who owns each unit of
truth — and is machine-checked.

**Status labels.** Each entry is tagged:

- **Enforced** — a test, lint or gate fails if it is broken. Trust it.
- **Held** — true in the code today, kept by convention and review. A change can break it silently.
- **Forward** — a rule for a seam that is banked but not yet built. It binds the work that lands
  on that seam; it is not a claim about today's behaviour.

**Adding an entry.** Add one when you find yourself writing "…and whoever does X next must also
do Y." Keep it to the contract and the reason; the implementation detail belongs in the code
comment. Delete an entry when the thing it protects stops existing — a register nobody trusts is
worse than none.

---

## 1. Retrieval and filing

### I-01 · A filter confines the candidate set; a score only reorders it — **Held**

A filter decides what retrieval is *allowed* to return (project scope, chat exclusion). A score
adjustment — recency decay, importance, reranking — may only change the order of what survived
the filter. Recency decay is deliberately floored so that even an infinitely stale document keeps
half its fused score: a stale document that is the only match for a rare term still surfaces.

**Why.** A boost with no floor is a filter wearing a disguise. It removes documents from the
answer without ever appearing in the filter list, so the "why wasn't this cited?" question has no
findable answer.

**Co-signers.** Any new retrieval signal picks a side explicitly. If it can drive a candidate's
score to (or near) zero, it is a filter — put it in `Filters` in `retrieval.rs` where it is
visible and explainable, not in the scoring pass.

### I-02 · One writer owns a document's filing — **Held**

`ingest::write_document_truth` is the single choke-point that files a document: it writes
project / tags / importance to the DB row **and** the vault front-matter, and emits the activity
event, as one act. Nothing else writes those fields.

**Why.** The vault is the portable truth and the DB is the queryable mirror. Two writers means
they disagree, and the disagreement only shows up on the next rebuild — long after the change
that caused it.

**Co-signers.** A new filing surface (a bulk action, an importer, an automation) calls
`write_document_truth`. It never issues `UPDATE documents SET project/tags/importance` directly,
and it never writes the vault front-matter itself.

### I-03 · A source-type block in vault metadata must round-trip — **Held**

Organisation writes **rebuild** the vault file's metadata block rather than patching it. A field
that the rebuild path cannot reproduce is therefore deleted by the next unrelated write.

**Why.** The loss is silent and total, and it happens on an action that had nothing to do with the
field — a user re-tags a document and a connector's pointer disappears.

**Co-signers.** Any PR adding a new front-matter key adds it to both the parse and the emit side,
and covers it with a round-trip test that writes → rebuilds → asserts the key survived.

---

## 2. Identity

Identity rules are the most expensive to retrofit, because the wrong key is only discovered once
real user data is keyed by it.

### I-04 · Calendar identity is the iCal UID, and an occurrence is UID + instant — **Held**

`calendar_events.uid` is the durable cross-provider anchor: it survives edits, re-syncs and the
mirror being rebuilt from scratch. A row's primary key does not — the mirror is refilled by
delete-and-reinsert, so any row id is scratch. A *specific occurrence* of a recurring event needs
the UID **and** the instance instant (`flags.instance_at`); the UID alone names the series.

**Co-signers.** Anything that stores a reference to a calendar event stores the UID, never the row
id. Anything that means "this Tuesday's standup, not every standup" stores the instant too.

### I-05 · Flags anchor only on durable ids — **Held**

`flags.anchor_kind` names the identity space (`calendar` → an iCal UID; `milestone` → a milestone
id) and resolution keys on `(anchor_kind, anchor, type)`. A flag never anchors on a title, a row
id, or anything else a re-sync can change.

**Co-signers.** A new anchor kind adds a `CHECK` arm and states, in the migration comment, which
durable id it points at and what guarantees that id's stability. On conflict, an assertion (the
user said so) outranks a detection (PM inferred it).

### I-06 · A new FK to `entities` has three sites, not one — **Enforced by review**

The entity mirror is rebuilt from the encrypted rules file at session open. A table pointing at
`entities` must therefore appear in **all three** places in `entities.rs`, in the same PR:

1. `merge_entities` — repoint rows from the merged-away entity.
2. The rebuild's NULL-out list — drop the pointer *before* the entities are deleted.
3. The re-resolve pass — restore the pointer through the rebuilt aliases (or state why not).

**Why.** Miss (2) and the rebuild dies on an FK violation **at boot**, on the next launch, with
no way into the app. `calendar_events.entity_id` is the worked example: nothing writes it yet, and
it is in the NULL-out list anyway, precisely so the first writer cannot cause that.

**Co-signers.** If a column is deliberately not re-resolved, the migration or the rebuild says so
in a comment. Silence reads as an oversight.

### I-07 · Three content-identity regimes, and a "duplicate" feature must name which — **Held**

PM hashes three different things and they are not interchangeable:

| Regime | What is hashed | Where |
|---|---|---|
| `documents.content_hash` | the **derived Markdown** body | `ingest.rs` |
| `photos.file_hash` | the **original bytes** | photo ingestion |
| `source_content_hash` | whatever the **provider reports** | index-only manifest |

Two files with identical text but different bytes match on the first and differ on the second.
A provider hash is comparable only with itself.

**Co-signers.** Any feature that says "duplicate", "unchanged" or "already have this" names the
regime it means, in the code and in the UI string. Never compare hashes across regimes.

### I-08 · Device identity has no single owner yet — **Forward**

PM currently holds two unrelated device notions: the OS device id baked into a local-folder file
key (`localfolder.rs`), and the vault's per-device metadata (`vault::ensure_device_meta`). Neither
is a general "which machine is this" identity, and neither should be borrowed as one.

**Co-signers.** The first work that genuinely needs a device identity (pairing, sync, per-device
settings) introduces **one** module that owns it, and migrates or explicitly leaves alone the two
existing notions — with a written reason. Do not grow a third.

---

## 3. The connector contract

Every source PM can index — and every consumer that reads indexed content — signs this. It is one
contract because the failure mode is always the same shape: a new source honours half of it, or a
new consumer forgets that index-only documents exist, and the result is a document that looks
indexed but answers nothing.

### I-09 · What a new source connector must ship — **Held**

1. **A stable external id.** The key must survive a rename and a move. If the provider's id is not
   stable, the connector defines its own correspondence and documents it.
2. **A reachability state**, not a deletion. Enumeration that no longer sees an item marks it
   unreachable; it does not delete the row. "We never saw it" and "the user deleted it" are
   different facts and must stay distinguishable.
3. **A partial-enumeration signal.** A walk that was truncated or hit an error reports the picture
   as incomplete, and an incomplete picture withholds any sweep that would reap rows.
4. **A body-fetch arm for rebuild.** Index-only rows hold a short stored summary as chunk text,
   never the document body. A rebuild that re-embeds from that summary silently downgrades the
   document's retrievability, so rebuild must re-fetch the body — or skip the row and say so.
5. **A promote-in-place path**, if the source can become a full local import: the manifest entry
   is stripped after the promoting transaction commits, and the DB row is left alone — it *is* the
   promoted document now.
6. **Honest usage accounting** on every arm that calls a model (see I-11).

### I-10 · What a consumer of indexed content must honour — **Held**

An index-only document's `chunks.content` holds a **placeholder summary**, not its text. Any
reader that treats chunk text as the document body — a reranker, a snippet renderer, a
summariser, an export — must either fetch the real body or handle the placeholder explicitly.
Treating it as body text buries index-only documents in exactly the surfaces meant to surface
them.

**Co-signers.** A new consumer of `chunks.content` states which of the two it does. There is no
third option, and "it works in testing" means the test corpus had no index-only rows.

---

## 4. Accounting, model access, and the DB lock

### I-11 · Every model call logs one usage row, through one writer — **Held**

`commands::log_usage` is the only writer of `usage_log`, and it tags each row with how the call
was served (provider, latency, fallback reason). It is best-effort — accounting must never fail a
model call — but a failed insert is **reported**, never silently swallowed.

**Why.** A silent `let _ =` around a rejecting `CHECK` hid missing cost data for months. The rule
now is: swallow the failure, print the reason.

**Co-signers.** A new arm that calls a model routes through `log_usage` with its own `kind`. A new
`kind` value goes in the column's `CHECK` list in the same PR.

### I-12 · A column `CHECK` list has exactly one owner in code — **Held**

Where a `CHECK (x IN (…))` constrains a column, one Rust type owns the string set (for example
`project_activity::Kind`), and every writer goes through it. Nothing types the literal by hand.

**Why.** A hand-typed literal that drifts from the `CHECK` fails at INSERT, at runtime, on the one
path nobody exercised. Relaxing a `CHECK` afterwards is a `writable_schema` text-patch — see the
AGENTS.md rule-3 gotcha — which is far more expensive than getting the set right once.

### I-13 · Models with no zero-data-retention endpoint are uncallable, and the carve-outs are visible — **Held**

PM pins `zdr: true` and `data_collection: "deny"` on every request, and the catalogue is filtered
to models with a ZDR endpoint. A model that cannot be served under ZDR is therefore not merely
discouraged — it cannot be called at all.

**Co-signers.** Any carve-out from that filter (today: router models, whose pins apply to whatever
they resolve to) is named in code with its reason. A silent carve-out is a privacy regression that
looks like a catalogue bug.

### I-14 · The DB mutex is not re-entrant — **Held**

Never take the connection guard while already holding it: the second take deadlocks the process
and presents as a frozen app, not an error. Helpers that quietly acquire the connection are the
usual cause, so check what a helper does before calling it inside a guarded block. And per
AGENTS.md rule 4, never hold the guard across an `.await`.

### I-15 · The vault walk changes in one place, and there are six of them — **Held**

Six production paths enumerate vault files, and a change to what a vault contains has to be
considered against all six:

| Walk | Owner |
|---|---|
| Rebuild sweep | `ingest::rebuild` |
| Key-migration re-encrypt | `ingest::convert_markdown` |
| Photo-originals re-encrypt | `ingest::convert_photo_originals` |
| Plaintext export ("never locked in") | `ingest::export_plaintext` |
| Backup tree collection | `backup::pack::collect_tree` |
| Teardown | `vault::migrate::delete_vault_artifacts` |

**Why.** A walk that misses a subdirectory does not error — it silently excludes those files from
whatever it was for. A re-key that skips a subdirectory strands the files in it permanently.

**Co-signers.** A PR that adds a file kind or a subdirectory to the vault walks this list and
states, per row, whether it applies. A new walker is added to the table.

---

## 5. Safety

### I-16 · Anything that sends, deletes, shares or spends is confirmed by default — **Held**

Actions that leave the machine or destroy data are user-initiated and confirmed, not inferred.
When PM proposes such an action, the proposal and the execution are separate steps with the user
in between. Default-on automation for this class needs an explicit product decision, not a
sensible-seeming default.

**Co-signers.** A new action in this class ships behind a confirmation, and its confirmation names
what will actually happen (which account, how many items, whether it can be undone).

### I-17 · Ingested and inbound content is data, never instructions — **Held**

This is AGENTS.md rule 6, repeated here because it is the invariant most often broken by *adding a
consumer* rather than by adding a source. Document text, connector payloads, calendar fields,
issue comments and chat imports are all untrusted.

**Co-signers.** A new surface that puts third-party text near a model puts it in the user message,
never the system prompt, and keeps the system prompt byte-identical regardless of the item being
processed.

---

## 6. Build tooling and dependencies

### I-18 · `scripts/` is zero-dependency, and each exception is a recorded decision — **Enforced**

Every file under `scripts/` imports Node built-ins (`node:*`) and repo-relative paths only. A
third-party package is allowed only as a named exception: an entry in `ALLOWED` in
`scripts/check-script-deps.mjs` giving the file, the specifier and the reason, with the package a
**devDependency** at an **exact** version. `just script-deps` fails on an unlisted import, on an
exception nothing imports any more, and on an exception that has drifted to a range pin or into
`dependencies`.

**Why.** Two reasons, and the first is not a matter of taste. Six of these scripts are PR gates that
run in pr.yml's `hygiene` job, and that job has **no `npm ci` step** — there is no `node_modules` on
that runner. A gate that imported a package would work perfectly on the maintainer's machine and
die only in CI. The second reason is that a build script is the easiest place for a dependency to
arrive unnoticed: nothing about it looks like a product decision. The rule is not "never" — one
exception is live and justified (`@huggingface/gguf`, for reading MoE expert counts out of a binary
GGUF header) — it is that adding one means editing a file that states the bar.

**Co-signers.** A PR that adds an import to `scripts/` either keeps it inside `node:*` or adds the
`ALLOWED` entry in the same PR, with the reason written where the next person will read it. A PR
that removes the last use of an allowed package drops its entry too. Loosening the pin, or moving
the package to `dependencies`, is a change to this invariant and needs its own argument.
