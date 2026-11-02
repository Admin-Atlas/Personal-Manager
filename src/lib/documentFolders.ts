// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Documents tab's folder views (#1030). The flat list shows everything at once; these sort the
// same documents into folders you open one at a time, the way Drive does:
//
//   • Folders — where each file came from. One top-level folder per place PM reads from (each Google
//     account's Drive, Shared with you, shared drives, each OneDrive, each tracked folder on this
//     device) and per kind of thing PM holds itself (files added to PM, photos, pinboard notes,
//     chats). Inside a connected place, the source's own folders, from the same stored trail the
//     breadcrumb under each title reads (#736).
//   • Projects — one folder per project. A file in several projects (#275) is in each of their
//     folders, the way a Drive file can sit in several.
//
// Pure: documents in, a tree out. The caller hands in the documents already sorted, and every
// folder's files keep that order, so the table's sorting carries into a folder unchanged.
//
// What a folder trail can't tell apart, this can't either: Drive allows two sibling folders with the
// same name, and PM stores each ancestor by name only, so their files share one folder here.

import type { Document } from "./types";

export type DocView = "all" | "folders" | "projects";

export interface FolderNode {
  /** Unique among its siblings; a path of these keys addresses a folder from the root. */
  key: string;
  name: string;
  /** A second line under the name: the account, or what the folder holds. */
  detail?: string;
  folders: FolderNode[];
  /** The documents directly in this folder, in the order they were handed in. */
  files: Document[];
  /** Every document in this folder and the folders under it. */
  total: number;
}

interface Segment {
  key: string;
  name: string;
  detail?: string;
  /** Top-level folders sort by this; deeper ones by name. */
  rank?: number;
}

/** The folders a document sits in, as a trail of path segments root first: one trail in the Folders
 *  view, one per project in the Projects view. */
function placements(doc: Document, view: Exclude<DocView, "all">): Segment[][] {
  if (view === "projects") {
    const names = [
      doc.project || "Unsorted",
      ...doc.linked_projects.filter((p) => p !== doc.project),
    ];
    return [...new Set(names)].map((name) => [{ key: `p:${name}`, name }]);
  }
  return [sourcePlacement(doc)];
}

const folderSegments = (trail: readonly string[]): Segment[] =>
  trail.map((name) => ({ key: `f:${name}`, name }));

/** The stored trail above a connector file: the full one when PM has it, else the one folder it has
 *  always known (as `documentBreadcrumb` falls back). */
function trailOf(doc: Document): string[] {
  return (
    doc.source_folder_path ?? (doc.source_parent_folder_name ? [doc.source_parent_folder_name] : [])
  );
}

function sourcePlacement(doc: Document): Segment[] {
  const id = doc.source_id;
  if (id?.startsWith("gdrive:")) {
    const rest = id.slice("gdrive:".length);
    if (rest.startsWith("swm:")) {
      return [
        { key: "gdrive:swm", name: "Shared with you", detail: "Google Drive", rank: 2 },
        ...folderSegments(trailOf(doc)),
      ];
    }
    // A shared drive's trail starts at the drive's own folder, so its name is the first crumb.
    if (rest.startsWith("sd:")) {
      return [
        { key: "gdrive:sd", name: "Shared drives", detail: "Google Drive", rank: 1 },
        ...folderSegments(trailOf(doc)),
      ];
    }
    const email = rest.split(":")[0];
    return [
      { key: `gdrive:${email}`, name: "Google Drive", detail: email, rank: 0 },
      ...folderSegments(trailOf(doc)),
    ];
  }
  if (id?.startsWith("onedrive:")) {
    const email = id.slice("onedrive:".length).split(":")[0];
    return [
      { key: `onedrive:${email}`, name: "OneDrive", detail: email, rank: 3 },
      ...folderSegments(trailOf(doc)),
    ];
  }
  if (id?.startsWith("local:")) {
    const folderKey = id.split(":")[1] ?? "";
    return [
      {
        key: `local:${folderKey}`,
        name: localRootName(doc) ?? "Folder",
        detail: "On this device",
        rank: 4,
      },
      ...folderSegments(trailOf(doc)),
    ];
  }
  if (id?.startsWith("note:")) return [{ key: "notes", name: "Pinboard notes", rank: 7 }];
  if (doc.source_type === "chat") return [{ key: "chats", name: "Chats", rank: 8 }];
  if (doc.source_type === "photo") return [{ key: "photos", name: "Photos", rank: 6 }];
  if (id == null) {
    return [
      { key: "added", name: "Added to PM", detail: "Files you dropped in or imported", rank: 5 },
    ];
  }
  return [{ key: "other", name: "Other", rank: 9 }];
}

/** A tracked folder's own name — the trail under it is relative to it, so it is the directory that
 *  many levels above the file. Null when PM can't tell (no full trail yet), and the caller says
 *  "Folder" rather than guess. */
function localRootName(doc: Document): string | null {
  if (!doc.external_ref || doc.source_folder_path == null) return null;
  const dirs = doc.external_ref.split(/[\\/]/).filter(Boolean).slice(0, -1);
  return dirs[dirs.length - 1 - doc.source_folder_path.length] ?? null;
}

interface Building {
  node: FolderNode;
  rank: number;
  children: Map<string, Building>;
  ids: Set<number>;
}

function newNode(seg: Segment): Building {
  return {
    node: { key: seg.key, name: seg.name, detail: seg.detail, folders: [], files: [], total: 0 },
    rank: seg.rank ?? 0,
    children: new Map(),
    ids: new Set(),
  };
}

const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** The folder tree for `view`, its root holding the top-level folders (and never files). */
export function buildFolderTree(
  docs: readonly Document[],
  view: Exclude<DocView, "all">,
  hiddenProjects: ReadonlySet<string> = new Set(),
): FolderNode {
  const root = newNode({ key: "", name: "" });
  for (const doc of docs) {
    for (const trail of placements(doc, view)) {
      if (view === "projects" && hiddenProjects.has(trail[0].name)) continue;
      let at = root;
      at.ids.add(doc.id);
      for (const seg of trail) {
        let next = at.children.get(seg.key);
        if (!next) {
          next = newNode(seg);
          at.children.set(seg.key, next);
        } else if (next.node.name === "Folder" && seg.name !== "Folder") {
          // A tracked folder named by a later file that knew its full trail.
          next.node.name = seg.name;
        }
        at = next;
        at.ids.add(doc.id);
      }
      at.node.files.push(doc);
    }
  }
  const finish = (b: Building, top: boolean): FolderNode => {
    const kids = [...b.children.values()].sort((x, y) =>
      top && x.rank !== y.rank
        ? x.rank - y.rank
        : byName.compare(x.node.name, y.node.name) ||
          byName.compare(x.node.detail ?? "", y.node.detail ?? ""),
    );
    b.node.folders = kids.map((k) => finish(k, false));
    b.node.total = b.ids.size;
    return b.node;
  };
  return finish(root, true);
}

/** The folder at `path`, with the folders on the way to it (root first, the folder itself last).
 *  A path that no longer leads anywhere — the folder emptied, or a remembered path from before —
 *  stops at the deepest folder that still exists. */
export function openFolder(
  root: FolderNode,
  path: readonly string[],
): { folder: FolderNode; trail: FolderNode[]; path: string[] } {
  const trail: FolderNode[] = [root];
  const kept: string[] = [];
  let at = root;
  for (const key of path) {
    const next = at.folders.find((f) => f.key === key);
    if (!next) break;
    trail.push(next);
    kept.push(key);
    at = next;
  }
  return { folder: at, trail, path: kept };
}

// --- remembered on this device, like the rest of `pm.documents.*` (see documentPrefs) -----------

export const DOC_VIEW_KEY = "pm.documents.view";
const FOLDER_PATH_KEY = (view: DocView) => `pm.documents.folder.${view}`;

export function readDocView(): DocView {
  try {
    const v = localStorage.getItem(DOC_VIEW_KEY);
    return v === "folders" || v === "projects" ? v : "all";
  } catch {
    return "all";
  }
}

export function writeDocView(view: DocView): void {
  try {
    localStorage.setItem(DOC_VIEW_KEY, view);
  } catch {
    /* best-effort — it just won't be remembered */
  }
}

/** The folder last open in `view`. A stored value that isn't a list of strings reads as the top. */
export function readFolderPath(view: DocView): string[] {
  try {
    const raw = localStorage.getItem(FOLDER_PATH_KEY(view));
    const v: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(v) && v.every((k) => typeof k === "string") ? v : [];
  } catch {
    return [];
  }
}

export function writeFolderPath(view: DocView, path: readonly string[]): void {
  try {
    localStorage.setItem(FOLDER_PATH_KEY(view), JSON.stringify(path));
  } catch {
    /* best-effort */
  }
}
