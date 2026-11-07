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
| `document_locations` | device | Every PLACE a connector found a document's file, one row each (#710). Device, not truth, and the distinction is the point: a `source_id` names a Drive account, a shared-drive id or an absolute path on **this machine**, none of which mean anything on another one — a second device reaches the same file through its own accounts and its own folders, and re-derives its own rows on its first sync. What IS truth here is the FILING, and that already lives on `documents` and travels in the `.pmindex` manifest. The manifest carries the sibling locations anyway, as a hint rather than a promise: a restore that lands them keeps a fold intact, and a restore that doesn't simply re-offers the pair to the duplicate pass. |
| `tags` | derived | The tag registry, both kinds. Project rows are re-derived from the vault (`project:` + `linked_projects:` front-matter) and the index-only manifest; group rows from `documents.tags`, which is itself re-derived from the vault's `tags:` line. Nothing here is authored, and `ingest::rebuild` reconstructs all of it — no model call. |
| `document_tags` | derived | Which documents carry which tags — project membership and free-form labels alike. Same rebuild path as `tags`, from the same front-matter; `ingest::write_document_truth` is its only writer, so it cannot diverge from the vault it indexes. |
| `chunks` | derived | Rebuilt by `ingest::rebuild` from the vault body. For index-only rows the content is a placeholder summary — see INVARIANTS.md I-10. |
| `chunk_vec` | derived | Embeddings. Already has a rebuild-on-mismatch path via the retrieval config stamp. Never portable: dimension and model must match the local registry. |
| `chunks_fts` | derived | FTS5 index over chunk text. |
| `conversations` | truth | Chat titles and project scope. The turn text is authoritatively in the vault; this row is not recomputable from it alone. |
| `messages` | truth | Bodies mirror the vault, but `citations`, `retrieved_chunk_ids` and `model` exist only here. |
| `chat_sessions` | derived | Session↔document mapping, rolling summary, cursors, last prompt size. Re-derivable — but regenerating the summary is a **billable** model call. |
| `settings` | mixed | Key-namespaced grab-bag. Truth: user preferences (retrieval k, reranking, backup frequency/retention), the per-Google-account calendar editing choice (`calendar_editing`, keyed by account email; editing also needs the account's keychain token, so a restore onto another machine shows it as needing Google's permission until reconnected) **and the three webview blobs that hold user content** — `milestone_ui` (the per-project milestone sort, keyed by PROJECT NAME, plus the show-completed toggle), `calendar_ui` (the hidden-calendar set, where a mod's calendar overlay is `mod:<id>` (the Pinboard's keeps its pre-mods `pm:pinboard`, so a saved set still applies) and an uninstall removes it, the last calendar an event was created in, whose ids are routinely the account's email address, and the Runway's named windows, `runway_windows`: user-written labels such as a trip, with their dates) and `backup_ui` (dismissed reconcile banners, keyed by destination + cloud account). Those three moved out of the webview's plaintext `localStorage` and are allowlisted to the webview in `settings.rs::WEBVIEW_PREFS`; they each get their own key because `project_ui` is rewritten whole on every divider drag. Truth too, briefly: `calendar_split_journal`, the record of a "this and following" calendar change from just before its first write until it's settled (Google calendar and event ids, which can hold the account's address, the original's repeat rules and start, and a plan digest; never a title or description). A `.pmbackup` carries it verbatim; PM reads Google again before acting on any entry, so a stale or restored one can only refuse a change until Google shows otherwise, and is never sent anywhere. Device: `mods_store_id`, the random name of this store's own mods folder on each machine (`data_dir/mods/<vault_id>/<store_id>/`), minted by its first install (or a startup that finds `mods` rows without one) and deleted by a restore, so a restored copy never shares a folder with the store it came from; a `.pmbackup` carries it, the restore drops it. Local model scan dir, external CLI paths, sync cursors, last-run timestamps, the On battery policy (`local_llm_power_threshold`, `local_llm_power_roles`, `local_llm_power_cloud_consent`) and `local_llm_battery_idle_minutes` — about this machine's battery and consent to bill this device's key; a `.pmbackup` still carries them verbatim. The Local AI switch (`local_ai_enabled`, `"true"`/`"false"`), whether this machine runs a local model at all: written once at a store's first open from whether any Local AI setting was stored, then only by the switch; a `.pmbackup` carries it verbatim, so a restore onto a machine with no model server shows Local AI on until it is switched off. Beside it, `local_llm_chat_model_emptied` and `local_llm_background_model_emptied` (`"true"` or absent): switching Local AI off cleared that role's chosen local model, so it waits for a model rather than falling through to the cloud; written only by switching off, deleted when a model or a routing is chosen for the role, or by the Local AI tab's reset; a `.pmbackup` carries them verbatim. Retired: `voice_enabled`, the old voice-input switch (`"true"`/`"false"`). Voice input was removed until a better speech engine replaces it, so nothing reads or writes the key now, but a store an earlier PM opened still holds the row (left in place, inert) and a `.pmbackup` from one carries it verbatim. The key must never be reused: a future speech-to-text takes a new key, so an old row can't switch it on for someone who never chose it. Derived: the briefing cache, layout caches, the retrieval config stamp, the filing seed vocabulary (`filing_seed_vocabulary` — machine-guessed from the unreviewed backlog, regenerable, and travels verbatim in a `.pmbackup`), `calendar_default_length`, each Google account's default event length (minutes), keyed by account email and read from Google when an account connects and at each sync (a read that fails keeps the last value; a disconnect clears it); a `.pmbackup` carries it verbatim. `calendar_auto_meet`, each Google account's "Automatically add Google Meet video conferences to events I create" (true or false), keyed by account email and read from Google's settings list the same way (a read that fails keeps the last value; a disconnect clears it); a `.pmbackup` carries it verbatim. **There is no key-prefix convention today** — see Open decisions. |
| `projects` | truth | Triage the user set: deadline, size, blocked-by, parent, importance, active date, archived. |
| `project_milestones` | truth | Multi-deadline project structure, incl. status and external-source anchoring. |
| `tasks` | truth | User to-dos: title, why, plan window, deadline (its own date, or one followed from a linked milestone or calendar event), status, lane, stack, parked, plus optional links to a project, a milestone and an event (by iCal UID). **No portable mirror exists:** the DB is the only home, and it travels only in a `.pmbackup`, verbatim. The pinboard Task list widget keeps just its project filter and its sort (a closed `{by, desc}` choice) in `settings['pinboard']`. |
| `task_stacks` | truth | Named groups of tasks (in sequence / in order / waiting on someone) with a note. Same home as `tasks`: the DB only, travelling in a `.pmbackup`. |
| `project_activity` | truth | An emit-only historical record of what happened when. Nothing can recompute a past event. |
| `project_activity_daily` | derived | Per-(project, day, kind) compaction of the above. |
| `corrections` | truth | The user's filing corrections, stamped with the pipeline version that produced the original. A learning signal; not recomputable. |
| `document_proposals` | derived | Explicitly a regenerable cache of the Review tab's AI proposals — dropped as a document leaves the queue. Re-deriving is a **billable** model call. |
| `duplicate_dismissals` | device | A decision the user made about their own library, not a fact about it: "I looked at these two and I am keeping both." Deliberately NOT truth — it is meaningless without the exact `documents.id` pair it references, which a rebuild on another machine would not reproduce, and re-offering a dismissed pair on a new machine is a harmless prompt rather than a loss. |
| `tag_proposals` | derived | Staging for a whole-library re-tag pass (#580) — a billable model call to re-derive, and empty in the steady state. Kept out of `document_proposals` on purpose: that one is committed by the review path, which writes project + importance together. |
| `retrieval_feedback` | truth | Relevance signal the user gave, stamped with the retrieval config it was given under. |
| `preferences` | truth | The structured preference model. **No portable mirror exists today** — this is the largest gap in the current picture; see Open decisions. |
| `entities` | truth | Already portable: mirrored to the encrypted rules file at the vault root and rebuilt from it at session open. The precedent every other truth table should follow. |
| `entity_aliases` | truth | Same mirror, same rebuild. |
| `flags` | mixed | Truth: `state`, `source='assertion'`, `user_confirmed`, `resolved_at` — the user's rulings. Derived: detections, re-derivable from calendar and milestone anchors. An assertion outranks a detection (INVARIANTS.md I-05). |
| `calendars` | mixed | Derived: the calendar list, re-enumerated from each provider, including the v57 editing facts (`access_role`, `time_zone`, `default_reminders`, `conference_types`, `data_owner`; `event_labels` from `calendars.get`). Truth: the per-calendar declarations — `selected`, `color`, `quiet`, `kind` (work/personal). |
| `calendar_events` | derived | A provider mirror, refilled by delete-and-reinsert, so no row id is stable. The v57 editing facts (`etag`, `event_type`, `organizer_self`, `locked`, `guests_can_modify`, `series_id`, `original_start`, `color_id`, `event_label_id`) are provider-derived too; Google stays the source of truth for an edit, so a restored `.pmbackup`'s copies are only refreshed, never trusted. One truth column: `kind_override`, the user's per-event escape hatch from the calendar's type. |
| `connector_sources` | device | Accounts are bound to this machine's keychain tokens and to locally-chosen folder ids. The portable counterpart is "the user has a Drive account connected", which is a re-connect prompt on a new device, not a synced row. |
| `shared_drive_access` | derived | Per-account shared-drive ownership bookkeeping; re-enumerated on sync. |
| `shared_with_me_access` | derived | Same, for the account-independent "Shared with me" corpus. |
| `photos` | derived | OCR text and visual descriptions re-derive from the originals in the vault — but only by re-running the model, so re-derivation is **billable** and slow. Carrying them is a cost decision, not a correctness one. |
| `spreadsheets` | derived | Satellite of the structured-data summary; re-derives from the source workbook. |
| `doc_layout` | derived | Cached 2-D map coordinates, already invalidated by a fingerprint. |
| `usage_log` | device | Per-device spend, latency and provider attribution. A cross-device total is a reporting question, not a sync one. `mod_id` (v60): the mod a row's spend was for, or NULL for PM's own calls; plain text with no FK, so the history outlives an uninstall and Usage & cost shows it as "removed mod". |
| `model_pricing` | derived | Refetchable price/capability cache. |
| `mods` | truth | The installed mods, one row each: version, publisher pin (tier, author, source repo and commit), signed or not, an install id (random, minted by each fresh install, kept by an update or a repair), where it came from, the package's own sha256, the verified manifest and file list (so a mod is rebuilt at startup from the row, never from what the disk claims), state and why, consent (D17), the AI cap with this month's settled spend, the notify day counter and the manifest's `projectRefs`. **The DB is the only home**, travelling in a `.pmbackup` verbatim; the files are not in it. A signed mod whose files are missing (a restore, a second device on a shared vault) parks until one click re-fetches exactly the pinned version and sha256; an unsigned one parks until its file is installed again. A developer's folder test mod (`installed_from = 'folder'`) is left out of backups. |
| `mod_grants` | truth | Each permission decision per mod (granted or denied; no row = not asked yet). Cascades from `mods`. Same home and travel as `mods`. |
| `mod_kv` | mixed | A mod's private storage, split by the `class` column, which comes from the manifest: `content` rows are truth (the Pinboard's board and notes; counted as user data, in backups unless "Also save mod data in my backups" is off), `cache` rows are derived (the mod rebuilds them; stripped from every backup snapshot, D4). `revision` refuses a write from a stale copy. Cascades from `mods`. |
| `mod_doc_data` | derived | Per-document mod rows, cache-class by rule in API v0 (a Rebuild re-creates every document with new ids). The owning mod recomputes them when it next runs; stripped from backup snapshots (D4). Cascades from `mods` and from `documents`, so a document delete reaches it with the mod closed. |
| `mod_ledger` | truth | Gift attribution: which mod made a task or milestone ("made by X"), by mod id and display name as plain text with no FK to `mods`, so it outlives an uninstall and a reinstall can edit its own again. Each row also keeps who the maker was (its publisher pin, whether it was signed, and its install id), because its `mods` row goes at uninstall. A gift is a mod's own (`mods::policy::made_it`) when the id and signedness match its installed row and then, for a signed maker, the pin (so a reinstall edits it again), or, for an unsigned maker, whose pin is only what its manifest claimed, the install id: neither another publisher's mod nor an unsigned file copying a removed mod's id and pin can edit it. Exactly one target per row (a nullable FK to `tasks` or `project_milestones`), cascading only from that target. Documents are attributed by their `source_id` prefix instead (`mod:<id>:<key>`; the Pinboard keeps `note:`). Same home as `tasks`: the DB, in a `.pmbackup`. |
| `mod_secrets` | truth | A mod's secrets, each bound to one host and one header from its manifest; typed by the user into a PM-drawn field and never handed to the webview. Vault-scoped: anyone who can open the vault (a shared vault's co-user included) can read them. **Stripped from every backup snapshot** (D4), so after a restore the user re-enters them, as with keychain credentials. Cascades from `mods`. |
| `mod_overlay` | derived | Calendar overlay entries (`{mod_id, id, date, label}`) a mod publishes, which PM draws without running mod code (the pop-out Calendar and the Runway included, D16). The mod re-publishes them from its own content storage when it runs. Cascades from `mods`. |
| `mcp_clients` | device | Connected MCP apps: name, the token's hash (the token is shown once and never stored) and scopes. About this machine's programs, so it is stripped from backups and the export, and cleared at restore commit. |
| `ai_reservations` | derived | In-flight worst-case AI cost per mod, held only while a call runs: settling deletes the row and adds the real cost to `mods.ai_spent_usd` in one transaction. Empty whenever no mod call is in flight; nothing to rebuild. Cascades from `mods`. |

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
| Frontend `localStorage` | device | **UI state only, and now by decision rather than by default.** What remains: view modes (calendar view/range/day-count/week-start/time-zones/open-on, the Agenda's Runway or List layout `pm.calendar.agendaMode`, whether the Runway shows its events `pm.calendar.runwayEvents`, Focus layout and upcoming mode, map grouping/cohesion/labels, briefing float mode); panel sizes (`pm.sidebar.frac`, both project fractions, the Focus split, the reader width, the briefing panel rect); the appearance axes (theme mode + accent + depth, the accessibility axes, the time zone the sunrise/sunset mode resolves against); capability flags (`pm:dev:mode`); the sidebar's tab placement (`pm.nav.placement`: the tabs pinned and hidden, PM's own tab ids or `mod:<id>`, never names); fold state (chat sections, calendar roster, briefing sidebar, the Runway's Before today column `pm.calendar.runwayPastOpen`, the task editor's Links group `pm.fold.tasks.editorLinks`); the panel/sort choices — both closed enumerations, panel ids and `{key,dir}`, never names; and the update/version markers. Plus `pm.calendar.cursor`, the day the user was last looking at, which **deliberately stayed** — it is view position, not content. The three keys that held user content (project names, calendar ids, cloud-account emails) moved to `settings` under `milestone_ui` / `calendar_ui` / `backup_ui`. |
| Sidecar venv + downloaded models | device | Runtime artifacts, provisioned per machine. Never portable. |
| `data_dir/mods/<vault_id>/<store_id>/` | device | Installed mods' files, `<id>/<version>/`, per store because each ledger lives in its own store and one machine can hold several: other vaults, and two stores of one vault (a restore keeps the vault id, and its copy can run beside the vault it came from). The store id is the store's `mods_store_id` setting, which a restore clears, so neither store ever sweeps, prunes or uninstalls the other's files. Never under `runtime/` (the plain Windows uninstall removes it while the ledger stays) and never in the vault root (it would join every vault walk, INVARIANTS.md I-15). The `mods` row is the truth; these bytes are served only while they match its sha256s. Not in a `.pmbackup`: a signed mod re-fetches its pinned version, an unsigned one waits for its file. Belongs to the vault-and-database class of Remove PM data (Mods spec §6), not a checkbox of its own; that removes `mods/<vault_id>/`, every store of the vault, as it removes every restored copy. |
| `data_dir/mods/.staging/` | device | Mod downloads in flight, on the same filesystem as the install folder (so the final rename can't fail across devices, and a large file never sits in tmpfs RAM). Verified and fsynced before the rename; anything left is swept at boot, after the store opens. |
| `data_dir/mods/<vault_id>/<store_id>/.quarantine/` | device | A mod folder the boot sweep found with no `mods` row, or a version folder its row doesn't name, moved here for 30 days instead of deleted (deletion only on provable absence). The sweep runs only as the writer, from a positive listing of `mods`, and moves nothing when it can't read the ledger. A copy whose files exactly match a row that names it again (a ledger briefly older than its files) is moved back at startup. |

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
