// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { NavItem } from "pm";

export const Sidebar = () => (
  <div style={{ width: 220, display: "flex", flexDirection: "column", gap: 2 }}>
    <NavItem active>Focus</NavItem>
    <NavItem onClick={() => {}}>Projects</NavItem>
    <NavItem
      onClick={() => {}}
      trailing={
        <span className="font-mono text-ink4" style={{ fontSize: 11 }}>
          14
        </span>
      }
    >
      Documents
    </NavItem>
    <NavItem onClick={() => {}}>Review</NavItem>
    <NavItem onClick={() => {}}>Settings</NavItem>
  </div>
);
