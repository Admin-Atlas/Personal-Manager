// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { ListRow, StatusBadge } from "pm";

export const Rows = () => (
  <div style={{ width: 420 }}>
    <ListRow
      title="Q3 board deck"
      meta="Due Tue · 12 files"
      trailing={<StatusBadge status="due_soon" />}
    />
    <ListRow
      title="Hiring — staff engineer"
      meta="Blocked on budget sign-off"
      trailing={<StatusBadge status="blocked" />}
    />
    <ListRow
      title="Reply to investor intro"
      meta="2 min"
      trailing={<StatusBadge status="quick_win" />}
    />
  </div>
);

export const ActiveAndInteractive = () => (
  <div style={{ width: 420 }}>
    <ListRow title="Inbox" meta="14 unfiled" active />
    <ListRow title="This week" meta="6 projects" onClick={() => {}} />
    <ListRow title="Someday" meta="23 parked" onClick={() => {}} />
  </div>
);
