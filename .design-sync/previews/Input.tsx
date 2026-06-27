// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Input } from "pm";

const col: React.CSSProperties = { display: "flex", flexDirection: "column", gap: 12, width: 320 };

export const Default = () => (
  <div style={col}>
    <Input placeholder="Search projects and documents…" />
  </div>
);

export const Filled = () => (
  <div style={col}>
    <Input defaultValue="Q3 board deck" />
  </div>
);

export const Disabled = () => (
  <div style={col}>
    <Input placeholder="Read-only" disabled />
  </div>
);
