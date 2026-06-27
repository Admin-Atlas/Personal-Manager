// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { HScroll, Card } from "pm";

const chip: React.CSSProperties = { minWidth: 150, padding: 12 };

// HScroll wraps wide content so a plain vertical wheel pans it sideways. Statically it reads as a
// horizontally-overflowing strip inside a fixed-width frame.
export const ProjectStrip = () => (
  <div style={{ width: 360 }}>
    <HScroll>
      <div style={{ display: "flex", gap: 12, paddingBottom: 8 }}>
        {["Fundraise", "Q3 board deck", "Hiring", "Product roadmap", "Customer calls"].map((t) => (
          <Card key={t} style={chip}>
            <div className="font-head text-ink" style={{ fontSize: 14 }}>
              {t}
            </div>
            <div className="font-mono text-ink4" style={{ fontSize: 11, marginTop: 6 }}>
              3 open
            </div>
          </Card>
        ))}
      </div>
    </HScroll>
  </div>
);
