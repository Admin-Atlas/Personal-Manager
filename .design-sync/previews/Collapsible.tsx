// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Collapsible } from "pm";

export const Open = () => (
  <div style={{ width: 380 }}>
    <Collapsible title="Sources" meta="4" defaultOpen>
      <ul
        className="text-ink3"
        style={{ fontSize: 13, lineHeight: 1.7, margin: "8px 0 0", paddingLeft: 18 }}
      >
        <li>Q2 board deck.pdf</li>
        <li>March investor update.md</li>
        <li>Revenue model.xlsx</li>
        <li>Customer interviews — notes</li>
      </ul>
    </Collapsible>
  </div>
);

export const Collapsed = () => (
  <div style={{ width: 380 }}>
    <Collapsible title="Reasoning" meta="hidden" defaultOpen={false}>
      <div className="text-ink3" style={{ fontSize: 13, paddingTop: 8 }}>
        Hidden until expanded.
      </div>
    </Collapsible>
  </div>
);
