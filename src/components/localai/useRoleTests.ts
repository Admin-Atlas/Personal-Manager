// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { activeLocalTest, testLocalLlm } from "../../lib/ipc";
import type { LocalRole } from "../../lib/localModelState";
import type { LocalTestResult } from "../../lib/types";

/** One role's last test: a result the backend returned, or a refusal it raised before running. */
export type RoleTest = { result: LocalTestResult | null; error: string | null };

/** The tab's "does this actually answer?" tests, one per role — what `useRoleTests` hands down. */
export interface RoleTests {
  /** The role whose test is in flight, or null. */
  testing: string | null;
  /** The last outcome per role. */
  tests: Record<string, RoleTest>;
  runTest: (role: LocalRole) => Promise<void>;
  clearTest: (role: LocalRole) => void;
}

/**
 * The role tests, held by the tab rather than by one section, so that any control that starts a test
 * or shows one running reads the same state — and they all agree on whether one is running.
 *
 * A test result is about one role's model on one server, and it stops being true the moment either
 * changes. The model half is the caller's to say (`clearTest`); the server half is `endpointEpoch`,
 * which the tab bumps whenever the stored endpoint or its token changes. A new epoch drops every
 * result and re-reads the backend's job — exactly what remounting the roles section used to do, back
 * when that section held this state and the epoch was its `key`.
 */
export function useRoleTests(endpointEpoch: number): RoleTests {
  // Which role's test is in flight, and the last outcome per role. Shared across the two rows
  // rather than held per row so a running test disables BOTH buttons: the backend refuses a second
  // one anyway, and a button that can only produce "a test is already running" is not a button
  // worth offering.
  const [testing, setTesting] = useState<string | null>(null);
  const [tests, setTests] = useState<Record<string, RoleTest>>({});

  // The server moved: forget what was proved against the old one. Done while rendering rather than
  // in an effect, so no frame ever shows the old server's pass under the new one's address.
  const [epoch, setEpoch] = useState(endpointEpoch);
  if (epoch !== endpointEpoch) {
    setEpoch(endpointEpoch);
    setTesting(null);
    setTests({});
  }

  /** Drop a test result the settings above it have just made untrue — the same rule the endpoint
   *  Check follows when the URL or token changes. A pass shown against a model you have since
   *  swapped is worse than no pass at all. */
  function clearTest(role: LocalRole) {
    setTests((t) => ({ ...t, [role]: { result: null, error: null } }));
  }

  /** Ask the role's model to actually answer something.
   *
   *  Everything the tab could check before this was metadata: the server answers, the weights are on
   *  disk, the id is in the list. The setups that fail fail at the step none of that covers — an id
   *  the server does not recognise, a chat template that returns an empty string, a model that
   *  starts loading and never finishes. The backend does the careful part (report what was already
   *  loaded, yield to chat, own nothing it did not load, record no health verdict) and OWNS the job,
   *  so this promise resolving is a convenience rather than the only way the answer arrives. */
  async function runTest(role: LocalRole) {
    setTesting(role);
    setTests((t) => ({ ...t, [role]: { result: null, error: null } }));
    try {
      const result = await testLocalLlm(role);
      setTests((t) => ({ ...t, [role]: { result, error: null } }));
    } catch (e) {
      setTests((t) => ({ ...t, [role]: { result: null, error: String(e) } }));
    } finally {
      setTesting(null);
    }
  }

  /** Adopt the backend's test job, on mount and while one is running.
   *
   *  The tab router unmounts this view on every switch and a test can legitimately take minutes, so
   *  without this a user who looked at another tab came back to a re-armed button, no sign anything
   *  was happening, and a backend still refusing a second test — with the answer they were waiting
   *  for already thrown away. The snapshot is the source of truth, exactly as it is for the pull.
   *
   *  Also on a new endpoint epoch: the reset above has just dropped what this view knew, and the job
   *  itself is the backend's and carries on regardless. */
  useEffect(() => {
    let cancelled = false;
    const adopt = () => {
      void activeLocalTest()
        .then((snap) => {
          if (cancelled || !snap) return;
          setTesting(snap.running ? snap.role : null);
          if (snap.result) {
            setTests((t) => ({
              ...t,
              [snap.role]: { result: snap.result, error: null },
            }));
          }
        })
        .catch(() => {
          /* a failed read leaves the view as it is; the next tick corrects it */
        });
    };
    adopt();
    // Only while something is running — a finished test needs no cadence at all.
    if (testing === null)
      return () => {
        cancelled = true;
      };
    const id = setInterval(adopt, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [testing, endpointEpoch]);

  return { testing, tests, runTest, clearTest };
}
