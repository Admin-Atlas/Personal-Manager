// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { StatusBadge } from "pm";

const wrap: React.CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 10,
  alignItems: "flex-start",
};

export const AllStatuses = () => (
  <div style={wrap}>
    <StatusBadge status="due_soon" />
    <StatusBadge status="blocked" />
    <StatusBadge status="quick_win" />
    <StatusBadge status="take_a_look" />
    <StatusBadge status="on_track" />
    <StatusBadge status="part_of" />
  </div>
);

export const CustomLabel = () => (
  <div style={wrap}>
    <StatusBadge status="part_of" label="Part of — Fundraise" />
    <StatusBadge status="due_soon" label="Due tomorrow" />
  </div>
);
