// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Textarea } from "pm";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 12, width: 360 };

export const Default = () => (
  <div style={col}>
    <Textarea rows={4} placeholder="Ask anything, or paste notes to file…" />
  </div>
);

export const Filled = () => (
  <div style={col}>
    <Textarea
      rows={4}
      defaultValue={
        "Draft the Q3 board deck.\n- Pull last quarter's metrics\n- Reuse the narrative from the March update"
      }
    />
  </div>
);
