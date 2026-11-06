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

### I-07 · Four identity claims, and a "duplicate" feature must name which — **Held**

PM makes four different identity claims and they are not interchangeable. Three are hashes:

| Regime | What is hashed | Where |
|---|---|---|
| `documents.content_hash` | the **derived Markdown** body | `ingest.rs` |
| `photos.file_hash` | the **original bytes** | photo ingestion |
| `source_content_hash` | whatever the **provider reports** | index-only manifest |

Two files with identical text but different bytes match on the first and differ on the second.
A provider hash is comparable only with itself.

The fourth is not a hash at all. **`document_locations` claims PROVENANCE** (#710): that two source
ids name the same underlying object — one Drive file reached through two accounts, or as both a
shared-drive item and a shared-with-me one. It is a claim about *where a file is*, not about what it
contains, and it is the only one of the four that can be true of two files whose bytes differ (one
copy edited since the other was last read) and false for two whose bytes are identical (two people
who each wrote the same invoice template).

**Co-signers.** Any feature that says "duplicate", "unchanged" or "already have this" names the
regime it means, in the code and in the UI string. Never compare hashes across regimes — and never
infer provenance from a hash match alone: identical content is EVIDENCE for a fold, never a proof of
one, and the fold is a claim PM should be able to point at a source id to justify.

**What that proof looks like (#711).** `locations::provenance_key` is the only thing in PM allowed
to assert the fourth claim, and it does so by deriving the PROVIDER's own global file id from a
source id — the one datum that is the same for every route to a file. Two consequences follow, and
both are load-bearing:

* A provider whose ids are not global answers `None`, and that is an answer, not a gap. OneDrive
  item ids are unique per drive; a bare-itemId key could collide across two accounts and merge two
  genuinely different files. **An unsound key is worse than no key** — a missed duplicate is a row
  the user scrolls past, a false one silently destroys a document.
* The key is compared with `=` against a stored column, never built into a `LIKE` pattern. Drive
  file ids legitimately contain `_`, which is a single-character wildcard, so `LIKE '…' || fileId`
  matches files that differ exactly where the underscore sits.

Anything proven this way is folded without asking (one document, many locations). Anything short of
proof stays in `duplicates.rs`, which only ever reports.

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

A walk's completeness is cleared by an unreadable *entry* as well as an unreadable *directory*.
`Path::is_file()` and `Path::is_dir()` are `metadata(..).map(..).unwrap_or(false)` — they return
`false` both for "this is not that" and for "the stat failed" — so **they are banned inside a vault
walk**. Use `ingest::probe`, which distinguishes them: only `NotFound` may be read as an absence,
because a deletion decision may only ever be made on a *provable* absence.

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

### I-19 · `safeUrl` gets the last word on a rendered URL — **Enforced**

react-markdown applies `urlTransform` **after** `processor.run`, so `markdown.tsx`'s `safeUrl` runs
*after* `rehype-sanitize`, not before it. That ordering is the whole point: the sanitizer's protocol
allowlist bails out "allowed" on any URL with no colon, so a **schemeless** target —
`//evil.example/x`, which the browser resolves against the page's own protocol and therefore straight
off-origin — passes it untouched. `safeUrl` is the only layer that can see that case, and its
protocol-relative guard must precede the `/`-prefix allowance, which would otherwise read the same
string as same-origin-relative and hand it back verbatim.

**Why.** The boundary used to justify itself with its own comment — "the sanitizer's protocol
allowlist above is what actually neutralises a hostile href" — which is false for exactly the class
no allowlist can match. The external-link interceptor is not a second net either: it keys on a URL
that *has* a scheme (`is-absolute-url` rejects `//host`), so it never sees one.

**Co-signers.** Anything added to `REHYPE_PLUGINS` goes *before* `rehypeSanitize`, which stays last.
Any edit to `safeUrl` keeps the `[/\\][/\\]` guard ahead of every allowance below it. A new surface
that renders ingested or model-authored content renders it through `<Markdown>` rather than
reproducing a `urlTransform` of its own. The one exception is a Google event's description, which
renders through `RichDescription` over `descriptionHtml.ts` (C19): it parses nothing as HTML, builds
React elements from an allow-list, and gives an `href` only to an address `linkTarget` takes
(absolute http, https or mailto, unchanged by `safeUrl`). `markdown.test.ts` pins the rule and the
`/path` counter-case; `markdown.render.test.tsx` pins the resulting `href` and `src` at the DOM, so a
regression is a red test rather than a review miss.

### I-20 · A re-key carries the vault's owner forward; only a confirmed transfer changes it — **Enforced**

`vault::prepare_shareable` mints a **whole new** `VaultMeta`, and `build_passphrase_meta` stamps
`owner_sid` with the account doing the minting. Until `OwnerOnRekey` existed that stamp simply stood,
so any account that could unlock a shared vault became its recorded owner by changing the passphrase
— silently, with no notice to anyone. Every caller now states which it means: `Keep` copies the
source meta's `owner_sid` and `ownership_transfer` forward on a Passphrase source, and `Claim` —
reachable only from a takeover the user confirmed — stamps the new owner and records an
`OwnershipTransfer` in the MAC-covered metadata. Ownership is never acquired as a side effect of
minting metadata.

**Why.** The carry-forward, not the gate, is what makes the ownership record worth anything.
`vault_ownership` returns `Unknown` for every shareable vault off Windows, for every vault created
before ownership was recorded, and for any SID-lookup hiccup — and the gate deliberately falls open
there, because a vault nobody can re-key is the one unrecoverable state in this design. If `Keep` did
not preserve the owner, that fall-open *would be* the takeover. With it, the worst a bypassed or
fallen-open gate can produce is "re-keyed, ownership unchanged".

**Co-signers.** A new path that mints a `VaultMeta` passes an explicit `OwnerOnRekey` and defaults to
`Keep`; nothing reaches `Claim` except an action the user confirmed. A path that sheds sharing
(`private_meta`, `normalize_adopted_meta`) clears `ownership_transfer` wherever it clears
`owner_sid`, or the record outlives the ownership it describes. The gate decisions stay in the pure
`gate_for` / `private_gate_for`, one layer below the command, where the whole rule is table-tested
without a vault, a keychain or a Windows account.

### I-21 · One conversion redacts a request URL, and no call site formats a raw `reqwest::Error` — **Held**

`impl From<reqwest::Error> for Error` in `error.rs` is the single place a failing request's URL is
reduced to scheme + host + port (`redact_url`), and `Error::Http` holds the error as `#[source]`
rather than `#[from]` precisely so that conversion cannot be short-circuited by a `?`. A call site
that formats the error itself — `e.to_string()`, or `format!("{e}")` into a `LocalFailure.detail` or
a warning string — puts the path, query, fragment and any userinfo straight back.

**Why.** Several of PM's request URLs *are* the secret: a private iCal feed URL and a Drive
resumable-upload session both authorise by URL, and `secret.rs` already names ICS bearer URLs as in
scope. These strings are user-visible — a toast, a connector's error field, a local-AI failure detail
— so a leak here is a leak into wherever the user pastes them.

**Co-signers.** A new HTTP call site lets `?` do the conversion. If it genuinely needs to build its
own message, it redacts through `error::redact_url` first and never interpolates the
`reqwest::Error` whole. Nothing re-adds `#[from]` to `Error::Http`.

### I-22 · Calendar writes have one path, and nothing on a model path reaches it — **Enforced**

Changing a Google event (#884) goes through `commands/calendar_edit.rs` and nowhere else: its three
commands refuse every window but the main one, its private `google_io` is the only code that sends
a calendar write, and the webview's wrappers are imported only by the editor's hook. No module that
calls a model names any of it, sends an authorised Google request, or holds a Calendar API URL; no
other module holds that URL beside anything that could send more than a read; and the write path
calls no model. `scripts/check-calendar-write-fence.mjs` (`just calendar-write-fence`) and an
`eslint.config.js` import rule fail the build when any of that slips, and the fence checks its own
lists of commands and wrappers against calendar_edit.rs, ipc.ts and the ESLint rule.

**Why.** Each step that would let model output edit someone's calendar is small and plausible on its
own: a chat helper that imports the wrapper "to fix a time", a briefing that calls the command, a
model path that builds a Calendar URL and sends it with the stored token. None would fail a test.
PM's model calls also have no tools today, which is the other half of this guarantee.

**Co-signers.** A new write (create, RSVP, a series edit) is a command in `calendar_edit.rs`, added
to the fence's `WRITE_COMMANDS` and the ESLint rule with its wrapper (the fence fails until it is),
and sends through `google_io`. AI editing (Stage 4) is a deliberate design of
its own, with the user approving each change before it is sent, and it changes this entry and the
fence on purpose rather than routing around them.

### I-23 · Nothing in the page parses HTML or writes an HTML string into it — **Enforced**

No code in `src/` hands text to an HTML parser (`DOMParser`, `createContextualFragment`,
`setHTML`, `parseHTMLUnsafe`) or writes markup into the page (`innerHTML`, `outerHTML`,
`insertAdjacentHTML`, `document.write`, `srcdoc`, `dangerouslySetInnerHTML`, `execCommand`'s
`insertHTML`); `createElement` and `execCommand` take only literal names. Google's formatted event
descriptions (#884, C19) are read by a character tokenizer (`src/lib/calendarEdit/descriptionTokens.ts`)
and the codec over it (`descriptionHtml.ts`), and are drawn as React elements or DOM nodes made one
by one. The `no-restricted-syntax` rule in `eslint.config.js` (`HTML_SINKS`) fails the lint on any
of them in shipped code, the calendar editor's hook included; only `*.test.{ts,tsx}` files are
exempt, in a block of their own. `src/lib/htmlSinkFence.test.ts` lints each form through ESLint's
own API.

**Why.** Each is an XSS sink CodeQL reads, and its highs block merges: CodeQL alert #31 came from
an inert `DOMParser` parse of a description, because the same field also takes text the user typed.
An inert parse is still a parse, and the next "quick" `innerHTML` for formatting would reopen it.

**Co-signers.** A new reader of formatted text extends the tokenizer and draws React elements or
nodes made one by one, never a parse. A new sink the rule doesn't name gets a selector in
`HTML_SINKS` and a case in the fence test. A link the drawn view (`RichDescription`) makes carries only
an address `descriptionHtml.ts`'s `linkTarget` takes (absolute http, https or mailto, unchanged by
`markdown.tsx`'s `safeUrl`, I-19). The editor's box also holds a phone number `keptLink` keeps, so the
link survives a save, and its click guard never opens one. Neither ever draws a string the user is
still typing.

### I-24 · A mod frame is `<iframe sandbox="allow-scripts" allow="">` in `ModFrame.tsx`, and no frame is anywhere else — **Enforced**

The only `<iframe>` in `src/` is the one in `src/components/mods/ModFrame.tsx`, written with the literal
attributes `sandbox="allow-scripts"` and `allow=""`, no spread, and nothing beyond `key`, `ref`, `src`,
`title`, `className`, `style` and `onLoad`. Nothing in the page makes a frame another way (`createElement`,
`jsx`, a tag name held in a string, `<object>`, `<embed>`, `<frame>`, `<portal>`, `<fencedframe>`), and
nothing reads or writes a frame's `sandbox`, `allow`, `allowfullscreen` or `credentialless`. The
`no-restricted-syntax` rule in `eslint.config.js` (`FRAME_SINKS`, `NO_IFRAME_ELEMENT`, `NO_IFRAME_NAME`,
`MOD_FRAME_SHAPE`) fails the lint on each, in TypeScript and in plain JavaScript under `src/`; only tests
are exempt, and the description tokenizer may name "iframe" as data. `src/lib/frameFence.test.ts` lints
each form through ESLint's own API. Over the frame's bridge, nothing a mod sends becomes an attribute, a
class, a style string or markup: a height request is a number `modBridge.ts` clamps, and a call runs under
the session PM opened for that frame, never one a message names.

**Why.** PM's commands have no per-window ACL, and Tauri counts the `pmmod` scheme as local (Mods spec
§1.4). The sandbox is what makes a mod's frame an opaque origin: on WebView2 a frame that lost it sends
`Origin: http://pmmod.localhost`, which Tauri treats as local, and reaches every command. Main's CSP frames
any mod path, so an unsandboxed frame written anywhere in the page is the hole. Anything in `allow` hands
the frame a permission the scheme's Permissions-Policy then has to take back.

**Co-signers.** A surface that shows a mod renders `ModFrame`, never a frame of its own. A change to the
frame's shape (a new attribute, a wider sandbox) changes this entry, `MOD_FRAME_SHAPE` and the fence test
together, and the sealed-frame regression job (`tests/sealed-frame`, `.github/workflows/sealed-frame.yml`)
re-measures it. Its harness (`frontend/harness.js`) mounts the probe from its own copy of the frame's
`sandbox` and `allow` values, and the app's `build.rs` refuses to build unless those equal the literals on
ModFrame's `<iframe>`, so the job can't measure a different shape from the one that ships. It speaks the
same bridge, and fails on any IPC escape or network leak on all three engines. What it doesn't run is
`mods/runtime.rs`'s serve decision (which file, and which as a document): its scheme is a small mirror of
that, and runtime.rs's unit tests are what hold it.

### I-25 · No mod ships in the installer — **Forward**

A mod reaches a user only as a signed `.pmmod` release asset, installed through the Mods tab (Mods
spec §1.1, §1.3). First-party mod source lives under `mods/` in this repo and is built into its own
`.pmmod` files: nothing under `mods/` is in any `bundle.resources`, and nothing in `src/` imports
`mods/`.

**Why.** An installed mod is one the ledger recorded after the verifier accepted it, under the
switches, the blacklist and the user's own uninstall. A mod inside the installer would skip every one
of those, and would ship to every user with each PM release.

**Co-signers.** The first mod (the Map, spec §3) lands with `check-mods.mjs` (spec §7), which fails on
a `mods/` path in a bundle's resources or a `src/` import of `mods/`; this entry is **Enforced** from
then on.

### I-26 · Every PM window is built through `mods::build_window` — **Held**

The main window (`tauri.conf.json` says `"create": false`, and `setup()` builds it), the briefing
panel (`tray.rs`) and the pop-out Calendar (`popout.rs`) are each built by `mods::build_window`
(`src-tauri/src/mods/mod.rs`). It adds the navigation guard, the download refusal, the top-frame and
WebRTC init scripts, WebView2's browser args (which point at PM's sinkhole proxy, or at a dead port
when the sinkhole couldn't be bound), and the engine switches (Mods spec §1.4). A test in that file
fails if the config entry is created by Tauri again.

**Why.** WebView2 refuses a second webview in the same user-data folder with different browser args,
and every PM window shares one. So on Windows a window built with a bare `WebviewWindowBuilder::build`
fails to open beside a sealed one. Everywhere else, it opens without the guard, which is the wall that
keeps a mod's page from becoming a top document holding Tauri's invoke key.

**Co-signers.** A new window goes through `build_window` with `Role::NoMods`. Only the main window is
`Role::Main`, the one that may load a mod frame and whose switches decide whether the engine is sealed.

### I-27 · A mod reaches the webview only as verified bytes, and its id only as a `ModId` — **Held**

Mod files are served by one handler, the `pmmod` scheme (`src-tauri/src/mods/runtime.rs`), and only
files of a package `mods::package::verify` accepted, registered through `InstalledMod::from_package`
and still matching their verified sha256 on disk. A mod's page renders only as a one-time entry URL
minted by `mod_frame_open`; every other HTML, XML or SVG response is an attachment, which WebKitGTK and
WebView2 turn into a refused download. WKWebView renders it all the same (wry's response delegate reads
no `Content-Disposition`), so on macOS the navigation guard alone keeps a non-entry page out of a frame.
A mod id is checked once, by `mods::ids::ModId::parse`, before it reaches a path, a header, a URL, a
`source_id` or a table row.

**Why.** PM's commands have no per-window ACL, and Tauri treats the `pmmod` scheme as local, so the
handler, the sealed frame and the navigation guard (I-26) are the only walls between a mod and every
command. A second way to put mod bytes in a webview (an `asset:` protocol, a blob URL from PM's page, a
file written outside the verified set) skips all of them, and an id that wasn't parsed can carry `..`,
a `;` into the CSP, or a Windows device name into a path.

**Co-signers.** The install path (Mods PR-1 slice 3) writes exactly a `VerifiedPackage`'s bytes, runs
the checks the verifier can't (index, blacklist, newer version, publisher pin, API range) and calls
`ids::check_tier`; storage, the dispatcher and the MCP server take a `ModId`, never a bare string; and
nothing else registers a scheme, protocol or URL that serves mod files.

### I-28 · A store a newer PM wrote is refused untouched, and the refusal never reaches a destructive path — **Held**

`migrations::run` refuses a store whose `user_version` is past this build's migrations (or negative)
before it writes anything, with `Error::Vault` coded `VaultFaultCode::NewerStore` and
`db::NEWER_STORE_MSG`. Every open goes through `db::open`, so boot, retry, unlock, join, restore,
switch-to-vault, the lock session's reopen and a vault move all get the same refusal. The recovery
paths that delete, replace or re-key key on a code or on `WRONG_KEY_OR_CORRUPT_MSG`, which the newer
message shares no words with: "Start fresh" refuses it by code (`wipe::reset_refusal`), "Start a new
empty store" accepts only `StoreMissing`, the boot open's stale-key branch clears a cached key only on
the brick message, a restore promotes nothing, re-homing a restore reads it as not pristine, and the
open-error screen offers only "Try again" (`VaultOpenError.tsx`). Tests pin the refusal itself, the
reset gate, restore, re-homing, unlock and the screen; the stale-key branch and "Start a new empty
store" are held by their code-or-literal gates, which need a keychain or a live app to exercise.

**Why.** Before this, `run` simply skipped a newer store and opened it as if current, which was safe
only while every migration was additive. Mods v60 is the first that isn't safe to ignore (its later
drop, the Pinboard move), and from here on a newer store is the user's whole library at a schema this
build can't name. Opening it would misread it; any recovery that treats it as damaged deletes it.

**Co-signers.** A new open path goes through `db::open`, never `open_keyed` plus its own migrate. A new
recovery that deletes, resets, re-keys or overwrites a store after an open failure branches on the
fault's code and refuses `NewerStore`, and never matches error text to decide. A migration that an
older build must not survive also bumps the backup `FORMAT_VERSION` (`backup/format.rs`).

### I-29 · Everything keyed to a mod cascades from `mods`, and attribution never does — **Held**

Uninstall is one `DELETE FROM mods` (Mods spec §1.3): grants, private storage, per-document rows,
secrets, overlay entries and AI reservations all `REFERENCES mods(id) ON DELETE CASCADE` (v60), and
`foreign_keys` is on for every connection `db::open` returns. Two things are deliberately outside it:
`mod_ledger` (which mod made a task or milestone) names the mod as plain text with who it was (its
publisher pin, whether it was signed, and its install id) and cascades only from its target, and
`usage_log.mod_id` is plain text. A gift is a mod's "own" (`mods::policy::made_it`) when its
`mod_ledger` row has the installed row's id and signedness, and then, for a signed maker, the same
pin (a reinstall edits its gifts again), or, for an unsigned maker, whose pin is only what its own
manifest claimed, the same install id, which only an update or a repair keeps. So neither another
publisher's signed mod nor any unsigned file that reuses a removed mod's id and copies its pin can
edit what that mod made.

**Why.** A per-mod table without the cascade is residue every uninstall leaves behind, invisibly,
secrets included. Attribution with the cascade would erase "made by X" on uninstall and lose a
reinstall's claim to edit its own gifts; spend history with it would make an uninstall rewrite the
Usage & cost totals.

**Co-signers.** A later table holding a mod's data references `mods(id) ON DELETE CASCADE` and gets its
row in SYNC-SET.md; a later record of what a mod made follows `mod_ledger` (text id, FK to the target
only). The cascade tests in `db/migrations.rs` gain the new table.

### I-30 · Mod files are written, moved and deleted only by the ledger, from the `mods` rows, by the writer — **Held**

Everything under `data_dir/mods` goes through `mods/disk.rs`, named by a `ModId`, a `VersionName`, a
checked vault id and a checked store id, and every change holds the runtime's ledger lock
(`mods/ledger.rs`). Each store has its own folder, `mods/<vault_id>/<store_id>/`: the store id is
random, kept in that store's `settings` (`mods_store_id`), and cleared by a restore, so a restored
copy (which keeps the vault id) never shares a folder with the store it came from. A `mods` row never
names files that aren't there: an install writes and fsyncs its files and renames them into place
before the row commits, and rolls them back if it doesn't. The boot sweep runs only after this
instance opens a store as its writer (`lock_session::holds_writer`), only from a positive listing of
the `mods` table, and not at all when that listing can't be read; a mod folder no row names, and a
version folder its row doesn't name, are moved to `.quarantine/` for 30 days, never deleted on sight
(only an update's own prune deletes, after its commit), a quarantined copy whose files exactly match a
row that names it again is put back, an exact copy in another store of the vault is copied in (never
moved, and on a blocking thread after the load, never inline in an open path), and a row whose files
are missing is parked, never deleted. Only the open store's folder is
ever changed, except to remove a store PM has just deleted, and then only once every row of the open
store was looked for there (a row this PM can't read back, or whose files differ, keeps those
folders); another store's is only read. Tests pin the
failure points, the read-error skip, the writer rule, the sweep, the recovery and two stores of one
vault side by side.

**Why.** A sweep that read "no rows" from a closed or curtained store would remove every mod, and an
unsigned one can't be re-fetched. A sweep made from another store's ledger, or an older copy of its
own, would do the same to the files that ledger doesn't name. A row that names missing files serves
nothing and can only be repaired; a folder no row names is residue that survives every uninstall.

**Co-signers.** A new writer of mod files (the Storage tab's "remove all mods", slice 7's auto-install)
goes through the ledger and its lock, never `std::fs` on its own. A new open path calls
`lock_session::engage` after the store opens, which is where the writer's mods load (or, when it isn't
the writer of an open store, where the registry is emptied); a path that takes the connection itself
calls `ledger::on_store_closed` before it can return, or, like `vault::migrate::relocate`, whenever it
returns with no store open. A new way to copy a store into a new one (restore is the only one
today) clears its store id with `ledger::forget_store_id`. A new "remove this vault's data" path
removes `mods/<vault_id>/` as `wipe::remove_vault_artifacts` does (spec §6) or, outside the wipe,
through `ledger::remove_deleted_vault` (the shared-vault delete and its acknowledgement), and a path
that deletes one store and keeps the vault (the re-homed restore's pristine home) removes that store's
folder through `ledger::remove_vacated_store`.

---

## 6. Build tooling and dependencies

### I-18 · `scripts/` is zero-dependency, and each exception is a recorded decision — **Enforced**

Every file under `scripts/` imports Node built-ins (`node:*`) and repo-relative paths only. A
third-party package is allowed only as a named exception: an entry in `ALLOWED` in
`scripts/check-script-deps.mjs` giving the file, the specifier and the reason, with the package a
**devDependency** at an **exact** version. `just script-deps` fails on an unlisted import, on an
exception nothing imports any more, and on an exception that has drifted to a range pin or into
`dependencies`.

The exact-version half has one documented escape hatch: an entry may set **`pinExempt`**, whose
value is the written reason the exact-pin rule does not apply to that package. The gate rejects an
exemption with no real reason, so waiving the pin costs the same sentence that adding the package
did. Nothing waives the `devDependency` half.

**Why.** Two reasons, and the first is not a matter of taste. Six of these scripts are PR gates that
run in pr.yml's `hygiene` job, and that job has **no `npm ci` step** — there is no `node_modules` on
that runner. A gate that imported a package would work perfectly on the maintainer's machine and
die only in CI. The second reason is that a build script is the easiest place for a dependency to
arrive unnoticed: nothing about it looks like a product decision. The rule is not "never" — two
exceptions are live and justified (`@huggingface/gguf`, for reading MoE expert counts out of a
binary GGUF header; and `vitest`, the repo's existing test runner, reached by a `scripts/` test the
same way every other test reaches it, and the one entry carrying a `pinExempt` reason) — it is that
adding one means editing a file that states the bar.

**Co-signers.** A PR that adds an import to `scripts/` either keeps it inside `node:*` or adds the
`ALLOWED` entry in the same PR, with the reason written where the next person will read it. A PR
that removes the last use of an allowed package drops its entry too. Loosening the pin is provided
for, but never silently — it takes a `pinExempt` reason in the same entry. Moving the package to
`dependencies` is a change to this invariant and needs its own argument.
