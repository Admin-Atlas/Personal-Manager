// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Review queue promises about a document that arrives while the view is busy.
//
// Three separate defects lived in this one view, and each of them is invisible in the markup:
//
//   1. `load()` merged live arrivals into the QUEUE but seeded the edits, the restored proposals and
//      the "still needs a suggestion" set from the PRE-MERGE query result. A document landing in the
//      few ms of the await was therefore rendered with no seeded edit, had the proposal that same
//      function had just cached painted away, and was left out of the backstop that would have asked
//      for one.
//
//   2. The live-arrival path had the same hole with no race at all: with suggestions off (the
//      default) an arriving row was never seeded by anything. An unseeded row is one keystroke from
//      a blank window — `updateEdit` spreads `undefined`, so the edit loses its tags and TagEditor
//      throws on `tags.map` inside a frameless window with no title bar left to close it.
//
//   3. Approve-all read the view's OWN `proposing` flag, which a background run never sets, and tore
//      the whole queue down wholesale. Scoping it without changing the teardown would have taken
//      held-back rows off the screen while leaving them unreviewed in the store.
//
// Each id space below is per-test on purpose: the view's hand-edit cache is module-level and outlives
// a `cleanup()`, so shared ids would let one case seed another.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const aiProviderStatus = vi.hoisted(() => vi.fn());
const cachedProposals = vi.hoisted(() => vi.fn());
const commitReview = vi.hoisted(() => vi.fn());
const listProjects = vi.hoisted(() => vi.fn());
const proposeMetadata = vi.hoisted(() => vi.fn());
const reviewQueue = vi.hoisted(() => vi.fn());

vi.mock("../lib/ipc", () => ({
  aiProviderStatus,
  cachedProposals,
  commitReview,
  listProjects,
  proposeMetadata,
  reviewQueue,
  // `lib/reviewProposals` imports the same module; nothing here calls these, but a missing export
  // would fail at import time rather than in the test that needed it.
  getDocument: vi.fn(),
  vaultStatus: vi.fn(async () => null),
}));

// The reader is mounted once at app scope; the row's title button only needs somewhere to send a
// click, and the real provider drags in the whole DocumentReader.
vi.mock("../lib/reader", () => ({
  useReader: () => ({ openReader: () => {}, current: null }),
}));

// The same stub the other component tests use: <Button>/<Input> reach for `useTheme`, and the real
// ThemeProvider pulls in IPC.
vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDepth: () => ({ depth: "standard", atLeast: () => true, showPower: false }),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ReviewView } from "./ReviewView";
import { pushLanding, resetDocumentFeed } from "../lib/documentFeed";
import { proposalCache, resetArrivalProposals, withProposalRun } from "../lib/reviewProposals";
import { REVIEW_AI_KEY } from "../lib/reviewPrefs";
import type { Document, MetadataProposal, ReviewDecision } from "../lib/types";

function doc(id: number, over: Partial<Document> = {}): Document {
  return {
    id,
    title: `Document ${id}`,
    source_path: null,
    ext: "md",
    byte_size: 100,
    chunk_count: 1,
    created_at: null,
    ingested_at: "2026-07-30T09:00:00Z",
    project: "Unsorted",
    linked_projects: [],
    tags: [],
    importance: null,
    reviewed: false,
    last_activity: null,
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
    source_author: null,
    source_last_modified_by: null,
    source_created_at: null,
    source_size_bytes: null,
    ...over,
  };
}

function proposal(over: Partial<MetadataProposal> = {}): MetadataProposal {
  return {
    project: "BIMUN 2026",
    tags: ["logistics"],
    importance: "high",
    reasoning: "Mentions the delegation's flight booking.",
    ...over,
  };
}

/** A promise whose settling this test controls — the load window, held open. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

/** The arrival feed coalesces on a 250 ms trailing timer; this is how a landing reaches the view. */
async function deliverLandings() {
  await act(async () => {
    await new Promise((r) => setTimeout(r, 320));
  });
}

function mount() {
  return render(<ReviewView onChanged={() => {}} onOpenSettings={() => {}} />);
}

/** The decisions of the nth `commitReview` call. */
function committed(call = 0): ReviewDecision[] {
  return commitReview.mock.calls[call][0] as ReviewDecision[];
}

beforeEach(() => {
  vi.clearAllMocks();
  resetArrivalProposals();
  resetDocumentFeed();
  proposalCache.clear();
  localStorage.clear();
  reviewQueue.mockResolvedValue([]);
  listProjects.mockResolvedValue([]);
  cachedProposals.mockResolvedValue([]);
  commitReview.mockResolvedValue(undefined);
  proposeMetadata.mockResolvedValue(undefined);
  aiProviderStatus.mockResolvedValue({ has_cloud_key: true, local_configured: false });
});

afterEach(() => {
  cleanup();
  resetArrivalProposals();
  resetDocumentFeed();
  proposalCache.clear();
  localStorage.clear();
});

describe("a document that lands during the load window", () => {
  it("is seeded from the proposal this load just cached, not from its own blank values", async () => {
    // The exact shape of the defect: the queue is merged (so the row renders) while the seed loop
    // read the query result (so the row rendered blank, on "Awaiting proposal…", with the model's
    // answer sitting one map away in the cache).
    const query = deferred<Document[]>();
    reviewQueue.mockReturnValue(query.promise);
    proposalCache.set(12, proposal({ project: "Delegation travel" }));

    mount();
    pushLanding(doc(12, { title: "Landed mid-load", project: "Unsorted" }));
    await act(async () => {
      query.resolve([doc(11, { title: "Already queued" })]);
    });

    expect(screen.getByText("Landed mid-load")).toBeTruthy();
    expect(screen.getByDisplayValue("Delegation travel")).toBeTruthy();
    // Its reasoning replaces the placeholder, so the row reads as answered rather than pending.
    expect(screen.queryByText("Awaiting proposal…")).toBeNull();
  });

  it("keeps its cached proposal instead of pruning it as no longer in the queue", async () => {
    // The #603 half, already correct — pinned so a refactor that re-unifies the two lists cannot
    // quietly unify them onto the query result.
    const query = deferred<Document[]>();
    reviewQueue.mockReturnValue(query.promise);
    const kept = proposal({ project: "Delegation travel" });
    proposalCache.set(22, kept);

    mount();
    pushLanding(doc(22));
    await act(async () => {
      query.resolve([doc(21)]);
    });

    expect(proposalCache.get(22)).toBe(kept);
  });

  it("joins the set the model is asked about when it has no proposal yet", async () => {
    // The load-path `missing` set is the BACKSTOP for whatever the live arrival path dropped — a
    // document that landed before a model was linked, or a drain that gave up. Reading the
    // pre-merge list put the one document most likely to need the backstop outside it.
    localStorage.setItem(REVIEW_AI_KEY, "true");
    const query = deferred<Document[]>();
    reviewQueue.mockReturnValue(query.promise);

    mount();
    pushLanding(doc(32, { title: "Landed mid-load" }));
    await act(async () => {
      query.resolve([doc(31)]);
    });

    expect(proposeMetadata).toHaveBeenCalledTimes(1);
    // Sorted: the order is `mergeLandings`' business (arrivals lead), not this test's.
    const asked = [...(proposeMetadata.mock.calls[0][1] as number[])].sort();
    expect(asked).toEqual([31, 32]);
  });
});

describe("a document that lands while the view is open", () => {
  it("survives being edited — an unseeded row blanks the whole window on the first keystroke", async () => {
    // With suggestions off (the default) nothing else ever seeds an arrival, so this needs no race.
    // Pre-fix, `updateEdit` spread an undefined base into `{project}` with no tags, and the next
    // render reached `tags.map` on undefined inside TagEditor — fatal, in a frameless window.
    mount();
    await act(async () => {});

    pushLanding(doc(42, { title: "Landed live", project: "Field notes", tags: ["travel"] }));
    await deliverLandings();

    const project = screen.getByDisplayValue("Field notes");
    fireEvent.change(project, { target: { value: "Delegation travel" } });

    expect(screen.getByDisplayValue("Delegation travel")).toBeTruthy();
    expect(screen.getByText("travel")).toBeTruthy();
  });

  it("does not lose a hand-edit when the same document is announced twice", async () => {
    mount();
    await act(async () => {});

    pushLanding(doc(52, { title: "Landed live", project: "Field notes" }));
    await deliverLandings();
    fireEvent.change(screen.getByDisplayValue("Field notes"), {
      target: { value: "Delegation travel" },
    });

    pushLanding(doc(52, { title: "Landed live", project: "Field notes" }));
    await deliverLandings();

    expect(screen.getByDisplayValue("Delegation travel")).toBeTruthy();
  });
});

describe("Approve all during a run this view did not start", () => {
  it("files what is ready and leaves the rest in the queue", async () => {
    // A background run (arrival batches, the post-sync sweep) never touches the view's own
    // `proposing`, so Approve used to be fully live during one. Scoping it is only half the fix: the
    // wholesale teardown would have swept the held-back row off the screen while the store still
    // had it as unreviewed — invisible, and unfilable without a reload.
    cachedProposals.mockResolvedValue([{ document_id: 61, proposal: proposal() }]);
    reviewQueue.mockResolvedValue([doc(61, { title: "Has a suggestion" })]);

    let release!: () => void;
    const run = withProposalRun(() => new Promise<void>((r) => (release = r)));
    try {
      mount();
      await act(async () => {});
      pushLanding(doc(62, { title: "Still waiting" }));
      await deliverLandings();

      const approve = screen.getByText("Approve 1 ready").closest("button")!;
      expect(approve.title).toBe("1 still waiting on a suggestion");
      await act(async () => {
        fireEvent.click(approve);
      });

      expect(commitReview).toHaveBeenCalledTimes(1);
      expect(committed().map((d) => d.document_id)).toEqual([61]);
      // The held-back row is still on screen AND still in the queue count.
      expect(screen.getByText("Still waiting")).toBeTruthy();
      expect(screen.getByText(/1 to review/)).toBeTruthy();
    } finally {
      release();
      await run;
    }
  });

  it("holds back that row's own Approve button, and says why", async () => {
    reviewQueue.mockResolvedValue([doc(71, { title: "Still waiting" })]);
    let release!: () => void;
    const run = withProposalRun(() => new Promise<void>((r) => (release = r)));
    try {
      mount();
      await act(async () => {});

      const row = screen.getByText("Still waiting").closest("li")!;
      const approve = row.querySelector<HTMLButtonElement>('[data-help="review-approve-one"]')!;
      expect(approve.disabled).toBe(true);
      // A disabled control has no other affordance, so the reason has to be the tooltip.
      expect(approve.title).toBe("Waiting for this file's suggestion");
    } finally {
      release();
      await run;
    }
  });
});

describe("what a committed decision claims about its baseline", () => {
  it("says there was no proposal when nothing was ever proposed", async () => {
    // `proposed_*` mirror the document's own values when no proposal exists, so a backend that
    // logged the difference logged a correction the user never made — on the default path, with
    // suggestions off. The flag is what lets it tell the two apart.
    reviewQueue.mockResolvedValue([doc(81, { title: "Filed by hand" })]);
    mount();
    await act(async () => {});

    fireEvent.change(screen.getByDisplayValue("Unsorted"), { target: { value: "Placements" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Approve all").closest("button")!);
    });

    expect(committed()[0].had_proposal).toBe(false);
    expect(committed()[0].project).toBe("Placements");
  });

  it("says there was one when the row shows a proposal", async () => {
    reviewQueue.mockResolvedValue([doc(91, { title: "Suggested" })]);
    cachedProposals.mockResolvedValue([{ document_id: 91, proposal: proposal() }]);
    mount();
    await act(async () => {});

    await act(async () => {
      fireEvent.click(screen.getByText("Approve all").closest("button")!);
    });

    expect(committed()[0].had_proposal).toBe(true);
    expect(committed()[0].proposed_project).toBe("BIMUN 2026");
  });
});
