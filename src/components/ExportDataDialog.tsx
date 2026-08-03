// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The export chooser (#712): what to take out of PM, and in what form.
//
// It replaces a single "Export all data…" button whose one behaviour — a zip whose Markdown is
// readable and whose database is still SQLCipher-encrypted — was described nowhere, so the user had
// to guess whether they were holding something readable or something safe. Naming the two axes makes
// that answer explicit instead of implied.
//
// Three of the four cells exist, and the fourth is refused rather than faked:
//
//             plain                                    encrypted
//   everything  today's zip: readable Markdown +        the existing .pmbackup — the same file a
//               the store still encrypted               backup writes, said plainly
//   documents   plaintext Markdown, decrypted           NOT OFFERED — restore hard-requires
//               (the "never locked in" escape hatch)    pm.sqlite and vault-meta.json, so a
//                                                       database-less archive needs a manifest
//                                                       schema decision, not a checkbox
//
// **An in-app modal, never a second OS window.** `WebviewWindowBuilder::build()` deadlocks Windows
// from a synchronous command or event handler, and a second window changes app-exit semantics (Tauri
// exits only when every window is DESTROYED). A chooser is not worth either.

import { useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";

import { createLocalBackup, exportAllData, exportPlaintextMarkdown } from "../lib/ipc";
import { Button, Dialog, Input, SegmentedControl } from "./ui";
import { PassphraseStrengthMeter } from "./PassphraseStrengthMeter";
import type { PassphraseScore } from "../lib/types";

type Scope = "everything" | "documents";
type Format = "plain" | "encrypted";

const SCOPES = [
  { value: "everything" as const, label: "Everything" },
  { value: "documents" as const, label: "Just my documents" },
];

const FORMATS = [
  { value: "plain" as const, label: "Plain" },
  { value: "encrypted" as const, label: "Encrypted" },
];

/** What the chosen combination actually produces, in the words the file will deserve.
 *
 *  Written per cell rather than assembled from two half-sentences: "plain" means something different
 *  either side of the scope switch — readable Markdown beside a still-encrypted database, versus
 *  Markdown with its at-rest protection deliberately stripped — and a generated sentence would have
 *  to hedge across both. */
function describe(scope: Scope, format: Format): string {
  if (format === "encrypted") {
    return "A single .pmbackup file, locked with a passphrase you choose. This is exactly what PM's backup writes — you can restore it on any machine, and PM cannot open it without that passphrase.";
  }
  if (scope === "everything") {
    return "A single .zip: your documents as readable Markdown, plus PM's store. The store stays encrypted inside the archive, so it only opens on a machine whose keychain holds this app's key — the Markdown is what you can read anywhere.";
  }
  return "A folder of plain .md files, decrypted. Nothing else — no projects, no chats, no search index. This is the escape hatch: your writing, readable by anything, with no PM required.";
}

export function ExportDataDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [scope, setScope] = useState<Scope>("everything");
  const [format, setFormat] = useState<Format>("plain");
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passScore, setPassScore] = useState<PassphraseScore | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  // The one refused cell. Disabling the segment would leave the user guessing why; saying it is the
  // whole difference between a limitation and a bug.
  const refused = scope === "documents" && format === "encrypted";
  const passphrasesMatch = pass.length > 0 && pass === confirm;
  // A scoring hiccup (null) never soft-locks the button — the backend floor is the real gate.
  const strongEnough = passScore?.acceptable !== false;
  const ready = refused ? false : format === "plain" || (passphrasesMatch && strongEnough);

  async function run() {
    setError(null);
    setMsg(null);
    // The plaintext-Markdown path picks its own folder in the BACKEND, deliberately: it writes
    // DECRYPTED content, so the destination must not be a path a compromised webview could
    // fabricate. Everything else takes a file path through the guarded save dialog.
    if (scope === "documents" && format === "plain") {
      setBusy(true);
      try {
        const res = await exportPlaintextMarkdown();
        if (res)
          setMsg(`Exported ${res.count} Markdown file${res.count === 1 ? "" : "s"} to ${res.dest}`);
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
      return;
    }

    const encrypted = format === "encrypted";
    let dest: string | null;
    try {
      dest = await saveFileDialog({
        defaultPath: encrypted ? "personal-manager.pmbackup" : "personal-manager-export.zip",
        filters: encrypted
          ? [{ name: "PM backup", extensions: ["pmbackup"] }]
          : [{ name: "Zip archive", extensions: ["zip"] }],
      });
    } catch (e) {
      setError(String(e));
      return;
    }
    if (!dest) return; // cancelled
    setBusy(true);
    try {
      if (encrypted) {
        // Runs detached and reports through the Backup tab's progress event — the same job, so it
        // reuses the same reporting rather than growing a second, quieter one here.
        await createLocalBackup(dest, pass);
        setMsg(`Writing your backup to ${dest}. Progress is on the Backup tab.`);
      } else {
        await exportAllData(dest);
        setMsg(`Exported to ${dest}`);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Export your data"
      subtitle="Choose how much to take, and in what form."
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            Close
          </Button>
          <Button variant="primary" onClick={() => void run()} disabled={busy || !ready}>
            {busy ? "Exporting…" : "Export…"}
          </Button>
        </>
      }
    >
      <div className="mt-3 space-y-3">
        {/* A plain label rather than `SectionLabel`: that primitive renders an `<h2>`, and the
            dialog's own title is already one — a second at the same level inside it would be a
            heading outline that says these two pickers are siblings of the dialog itself. The
            group is named through `ariaLabel`, which is the typed half of the primitive's naming
            union and matches the visible words exactly. */}
        <div>
          <p className="text-xs text-ink4">How much</p>
          <SegmentedControl
            className="mt-1"
            ariaLabel="How much"
            options={SCOPES}
            value={scope}
            onChange={setScope}
          />
        </div>
        <div>
          <p className="text-xs text-ink4">What form</p>
          <SegmentedControl
            className="mt-1"
            ariaLabel="What form"
            options={FORMATS}
            value={format}
            onChange={setFormat}
          />
        </div>

        <p className="text-xs text-ink3">{describe(scope, format)}</p>

        {refused && (
          <p className="text-xs text-[var(--st-due)]" role="alert">
            PM can&rsquo;t make an encrypted archive of the documents alone. Restoring one needs
            PM&rsquo;s store and the vault&rsquo;s own key file, so an archive without them would be
            a file you could never restore. Choose <span className="font-medium">Everything</span>{" "}
            for an encrypted copy, or <span className="font-medium">Plain</span> for the documents
            on their own.
          </p>
        )}

        {format === "encrypted" && !refused && (
          <div className="space-y-2">
            <p className="text-xs text-ink4">
              Choose a passphrase for the archive. It is not your vault passphrase and PM does not
              store it &mdash; without it the archive cannot be opened, by anyone, including PM.
            </p>
            <Input
              type="password"
              placeholder="Passphrase for this archive"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
            />
            <Input
              type="password"
              placeholder="Confirm passphrase"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
            <PassphraseStrengthMeter passphrase={pass} onScored={setPassScore} />
          </div>
        )}

        {error && (
          <p role="alert" className="break-all text-xs text-[var(--st-due)]">
            {error}
          </p>
        )}
        {msg && <p className="break-all text-xs text-ink4">{msg}</p>}
      </div>
    </Dialog>
  );
}
