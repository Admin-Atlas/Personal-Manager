# SYNC-SET.md — who owns each unit of truth

PM is local-first, and for a long time "the vault plus its rules file is the portable part" was a
good enough answer. It is no longer accurate. Several features since have chosen the **database**
as their home for truth — structured preferences, chat sessions, flag assertions, the activity
log, retrieval feedback — and a copy-the-Markdown-across model would strand every one of them on
one machine, silently, with no error and no empty state to notice.

This file fixes the answer per table, in advance of the work that will need it. It is **checked by
`just sync-set`**: every table the schema creates must appear below with a class, so a new table
cannot land unclassified.

Deciding this now is cheap — it is one row per table. Deciding it later means excavating
ownership out of a dozen shipped features while a sync implementation waits.

## Classes

| Class | Meaning | On a second device |
|---|---|---|
| **truth** | User-authored or user-owned meaning. Losing it is data loss; no process can recompute it. | Must arrive. |
| **derived** | Reconstructable from truth by a documented rebuild path. | Rebuilt locally, never shipped. |
| **device** | Genuinely about *this* machine — paths, hardware, per-device credentials, local spend. | Stays put. |
| **mixed** | Truth and non-truth in one table, split by column or key. The split is named in Notes. | Only the truth part travels. |

Two rules that fall out of the classes:

- **`derived` is a promise, not a label.** Marking a table derived asserts that a rebuild path
  exists *and is reachable from the UI*. If re-deriving costs a model call, say so in Notes — it
  is still derived, but the cost is a product decision, not a free lunch.
- **`device` is not "unimportant".** A device-local row often has a truth-shaped counterpart that
  does need to travel (a connector *account* is portable; its OAuth token is not). Notes name the
  counterpart.

## The register

| Table | Class | Notes |
|---|---|---|
| `documents` | truth | Filing is truth: `project` (the HOME project), `tags`, `importance`, `reviewed`, `entity_id`. Device-local columns: `source_path`, `vault_path`, `source_id` (a local connector row). The body is in the vault, not here. A document's *other* project memberships are in `document_tags`. |
| `tags` | derived | The tag registry, both kinds. Project rows are re-derived from the vault (`project:` + `linked_projects:` front-matter) and the index-only manifest; group rows from `documents.tags`, which is itself re-derived from the vault's `tags:` line. Nothing here is authored, and `ingest::rebuild` reconstructs all of it — no model call. |
| `document_tags` | derived | Which documents carry which tags — project membership and free-form labels alike. Same rebuild path as `tags`, from the same front-matter; `ingest::write_document_truth` is its only writer, so it cannot diverge from the vault it indexes. |
| `chunks` | derived | Rebuilt by `ingest::rebuild` from the vault body. For index-only rows the content is a placeholder summary — see INVARIANTS.md I-10. |
| `chunk_vec` | derived | Embeddings. Already has a rebuild-on-mismatch path via the retrieval config stamp. Never portable: dimension and model must match the local registry. |
| `chunks_fts` | derived | FTS5 index over chunk text. |
| `conversations` | truth | Chat titles and project scope. The turn text is authoritatively in the vault; this row is not recomputable from it alone. |
| `messages` | truth | Bodies mirror the vault, but `citations`, `retrieved_chunk_ids` and `model` exist only here. |
| `chat_sessions` | derived | Session↔document mapping, rolling summary, cursors, last prompt size. Re-derivable — but regenerating the summary is a **billable** model call. |
| `settings` | mixed | Key-namespaced grab-bag. Truth: user preferences (retrieval k, reranking, backup frequency/retention) **and the three webview blobs that hold user content** — `milestone_ui` (the per-project milestone sort, keyed by PROJECT NAME, plus the show-completed toggle), `calendar_ui` (the hidden-calendar set, whose ids are routinely the account's email address) and `backup_ui` (dismissed reconcile banners, keyed by destination + cloud account). Those three moved out of the webview's plaintext `localStorage` and are allowlisted to the webview in `settings.rs::WEBVIEW_PREFS`; they each get their own key because `project_ui` is rewritten whole on every divider drag. Device: local model scan dir, external CLI paths, sync cursors, last-run timestamps. Derived: the briefing cache, layout caches, the retrieval config stamp, the filing seed vocabulary (`filing_seed_vocabulary` — machine-guessed from the unreviewed backlog, regenerable, and travels verbatim in a `.pmbackup`). **There is no key-prefix convention today** — see Open decisions. |
| `projects` | truth | Triage the user set: deadline, size, blocked-by, parent, importance, active date. |
| `project_milestones` | truth | Multi-deadline project structure, incl. status and external-source anchoring. |
| `project_activity` | truth | An emit-only historical record of what happened when. Nothing can recompute a past event. |
| `project_activity_daily` | derived | Per-(project, day, kind) compaction of the above. |
| `corrections` | truth | The user's filing corrections, stamped with the pipeline version that produced the original. A learning signal; not recomputable. |
| `document_proposals` | derived | Explicitly a regenerable cache of the Review tab's AI proposals — dropped as a document leaves the queue. Re-deriving is a **billable** model call. |
| `tag_proposals` | derived | Staging for a whole-library re-tag pass (#580) — a billable model call to re-derive, and empty in the steady state. Kept out of `document_proposals` on purpose: that one is committed by the review path, which writes project + importance together. |
| `retrieval_feedback` | truth | Relevance signal the user gave, stamped with the retrieval config it was given under. |
| `preferences` | truth | The structured preference model. **No portable mirror exists today** — this is the largest gap in the current picture; see Open decisions. |
| `entities` | truth | Already portable: mirrored to the encrypted rules file at the vault root and rebuilt from it at session open. The precedent every other truth table should follow. |
| `entity_aliases` | truth | Same mirror, same rebuild. |
| `flags` | mixed | Truth: `state`, `source='assertion'`, `user_confirmed`, `resolved_at` — the user's rulings. Derived: detections, re-derivable from calendar and milestone anchors. An assertion outranks a detection (INVARIANTS.md I-05). |
| `calendars` | mixed | Derived: the calendar list, re-enumerated from each provider. Truth: the per-calendar declarations — `selected`, `color`, `quiet`, `kind` (work/personal). |
| `calendar_events` | derived | A provider mirror, refilled by delete-and-reinsert, so no row id is stable. One truth column: `kind_override`, the user's per-event escape hatch from the calendar's type. |
| `connector_sources` | device | Accounts are bound to this machine's keychain tokens and to locally-chosen folder ids. The portable counterpart is "the user has a Drive account connected", which is a re-connect prompt on a new device, not a synced row. |
| `shared_drive_access` | derived | Per-account shared-drive ownership bookkeeping; re-enumerated on sync. |
| `shared_with_me_access` | derived | Same, for the account-independent "Shared with me" corpus. |
| `photos` | derived | OCR text and visual descriptions re-derive from the originals in the vault — but only by re-running the model, so re-derivation is **billable** and slow. Carrying them is a cost decision, not a correctness one. |
| `spreadsheets` | derived | Satellite of the structured-data summary; re-derives from the source workbook. |
| `doc_layout` | derived | Cached 2-D map coordinates, already invalidated by a fingerprint. |
| `usage_log` | device | Per-device spend, latency and provider attribution. A cross-device total is a reporting question, not a sync one. |
| `model_pricing` | derived | Refetchable price/capability cache. |

## Beyond the database

The DB is not the whole picture, and three of these already solve the portability problem the
tables have not:

| Surface | Class | Notes |
|---|---|---|
| `vault/*.md` (and `.md.pmenc`) | truth | The portable body + front-matter. This is what "the vault travels" has always meant. |
| `vault/photos/` originals | truth | The original bytes behind photo documents. |
| `entities.pmrules` (vault root, encrypted) | truth | The portable mirror for entities + aliases. Re-encrypted on rekey. |
| `.pmindex` manifest (vault root, encrypted) | truth | The portable mirror for index-only pointers; `index_only::rebuild_from_manifest` restores the DB rows from it. Index-only sources are **already** portable — this corrects the older assumption that they had no export. |
| OS keychain | device | API key, DB key, OAuth tokens, feed URLs. Never travels, by design. |
| Frontend `localStorage` | device | **UI state only, and now by decision rather than by default.** What remains: view modes (calendar view/range/day-count/time-zones/open-on, Focus layout and upcoming mode, map grouping/cohesion/labels, briefing float mode); panel sizes (`pm.sidebar.frac`, both project fractions, the Focus split, the reader width, the briefing panel rect); the appearance axes (theme mode + accent + depth, the accessibility axes, the time zone the sunrise/sunset mode resolves against); capability flags (`pm:dev:mode`); fold state (chat sections, calendar roster, briefing sidebar); the panel/sort choices — both closed enumerations, panel ids and `{key,dir}`, never names; and the update/version markers. Plus `pm.calendar.cursor`, the day the user was last looking at, which **deliberately stayed** — it is view position, not content. The three keys that held user content (project names, calendar ids, cloud-account emails) moved to `settings` under `milestone_ui` / `calendar_ui` / `backup_ui`. |
| Sidecar venv + downloaded models | device | Runtime artifacts, provisioned per machine. Never portable. |

## Checklist for a PR that adds a table

1. Add a row to the register above with a class. `just sync-set` fails without it.
2. If the class is **truth**, say where the portable copy lives. If the answer is "nowhere yet",
   write that in Notes — an honest gap is checkable; an unstated one is not.
3. If the class is **derived**, name the rebuild path, and flag it if re-deriving costs a model call.
4. If the class is **mixed**, name the split by column or key prefix. Do not leave it implied.
5. If the table has an FK to `entities`, also do the three things in INVARIANTS.md I-06.

## Open decisions

These are product calls, recorded here so they are decided once rather than re-litigated per
feature. None is implied by the classifications above.

1. **Export format per truth table.** The recommendation is to extend the existing pattern rather
   than invent a wire format: an encrypted sidecar file at the vault root, per truth domain, as
   `entities.pmrules` and `.pmindex` already do. It survives a vault copy, it is covered by the
   rekey path, and it needs no server.
2. **`preferences` has no portable mirror.** It is truth, the DB is its only home, and its own
   migration assumed the DB travels with the vault. This is the first gap to close, and the
   `.pmrules` shape fits it directly.
3. **Billable re-derivation.** Chat summaries, review proposals and photo OCR are all derived but
   cost money to rebuild. Carry them, or accept the re-bill on a new device?
4. **Frontend `localStorage`** — **RESOLVED (3.114.0-alpha).** The slice that was never a decision
   is now one. Three keys held user content — a milestone sort map keyed by project name, a
   hidden-calendar set of ids that are routinely email addresses, and one dynamically-named
   dismissal key per cloud account — and they moved into the encrypted `settings` table
   (`milestone_ui`, `calendar_ui`, `backup_ui`), so they are encrypted at rest, travel in a
   `.pmbackup`, and go away with PM's data. Everything left is genuine chrome and stays device-local
   **on purpose**: theme, accent and depth per machine is the right default for a display
   preference, and so are panel sizes and view modes. The remaining question is narrower than the
   original one — whether the appearance axes should *optionally* follow the user across machines —
   and it is a product call, not a correctness gap.
5. **`usage_log` aggregation.** Device-local is right for the rows; whether a combined spend total
   across devices is wanted is a separate question.
