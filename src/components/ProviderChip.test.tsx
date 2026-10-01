// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer's local-endpoint pill. The load-bearing test is the zero-pixel one: a cloud-only
// user (no configured endpoint) must see literally nothing rendered. The state labels are pinned so
// the copy can't silently drift from the sidebar line (both read `localEndpointState`).

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProviderChip } from "./ProviderChip";
import type { LocalLlmStatus } from "../lib/types";
import { LOCAL_STATE_LABEL } from "../lib/localStatus";
import { INERT_POWER_VIEW } from "../lib/powerRoute";

const st = (over: Partial<LocalLlmStatus>): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: true,
  chat_local_model: null,
  background_local_model: null,
  served_window: null,
  served_window_proven: false,
  window_source: null,
  chat_answering: false,
  background_answering: false,
  chat_loaded: null,
  background_loaded: null,
  chat_released: false,
  background_released: false,
  power: INERT_POWER_VIEW,
  ...over,
});

describe("ProviderChip", () => {
  it("renders nothing when status is null (fresh boot / cloud-only)", () => {
    const { container } = render(<ProviderChip status={null} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders nothing when no endpoint is configured (the zero-pixel contract)", () => {
    const { container } = render(<ProviderChip status={st({ configured: false })} />);
    expect(container.firstChild).toBeNull();
  });

  it("shows the three states once an endpoint is configured", () => {
    expect(
      render(<ProviderChip status={st({ reachable: true })} />).container.textContent,
    ).toContain("connected");
    expect(
      render(<ProviderChip status={st({ in_cooldown: true })} />).container.textContent,
    ).toContain("using cloud");
    expect(
      render(<ProviderChip status={st({ reachable: false })} />).container.textContent,
    ).toContain("unreachable");
  });

  it("words each state exactly as the sidebar line does", () => {
    // The substring assertions above are what let the two surfaces drift: the chip said
    // "resting (using cloud)" and the sidebar "resting - using cloud" for a release and a half,
    // and "using cloud" passed for both. A shared classifier does not make shared copy — the shared
    // TABLE does, and this pins the chip to it exactly rather than approximately.
    for (const [state, over] of [
      ["connected", { reachable: true }],
      ["resting", { in_cooldown: true }],
      ["unreachable", { reachable: false }],
    ] as const) {
      const { container } = render(<ProviderChip status={st(over)} />);
      expect(container.textContent).toContain(LOCAL_STATE_LABEL[state]);
    }
  });

  it("says chat is on the cloud while the On battery policy has moved it, in neutral ink", () => {
    // "Local · connected" would be true of the server and false of the reply. And the status
    // colours are the health family: `--st-look` is what a FAILURE wears, and a policy the user
    // chose must never borrow it.
    const { container } = render(
      <ProviderChip
        status={st({
          power: {
            ...INERT_POWER_VIEW,
            state: "battery_low",
            chat: { route: "cloud", blocked: null, local_model: "gemma3:4b" },
          },
        })}
      />,
    );
    const pill = container.firstElementChild as HTMLElement;
    expect(pill.textContent).toBe("Cloud·on battery");
    expect(pill.textContent).not.toContain("connected");
    expect(pill.getAttribute("style")).toContain("--ink3");
    expect(pill.getAttribute("style")).not.toContain("--st-look");
    expect(pill.title).toMatch(/Settings → Local AI → On battery/);
  });

  it("keeps the endpoint pill for every route that isn't a move", () => {
    for (const route of ["unchanged", "needs_consent", "kept_local"] as const) {
      const { container } = render(
        <ProviderChip
          status={st({
            power: {
              ...INERT_POWER_VIEW,
              chat: { route, blocked: null, local_model: "gemma3:4b" },
            },
          })}
        />,
      );
      expect(container.textContent).toContain("Local");
      expect(container.textContent).toContain("connected");
    }
  });
});
