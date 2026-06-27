// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Select } from "pm";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 12 };

export const Default = () => (
  <div style={col}>
    <Select defaultValue="claude-opus">
      <option value="claude-opus">Claude Opus 4.8</option>
      <option value="claude-sonnet">Claude Sonnet 4.6</option>
      <option value="claude-haiku">Claude Haiku 4.5</option>
    </Select>
  </div>
);

export const Disabled = () => (
  <div style={col}>
    <Select defaultValue="claude-opus" disabled>
      <option value="claude-opus">Claude Opus 4.8</option>
    </Select>
  </div>
);
