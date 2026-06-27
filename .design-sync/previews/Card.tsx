// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Card } from "pm";

export const CardVariant = () => (
  <div style={{ width: 360 }}>
    <Card style={{ padding: 16 }}>
      <div className="font-head text-ink" style={{ fontSize: 15, marginBottom: 4 }}>
        Q3 board deck
      </div>
      <div className="text-ink3" style={{ fontSize: 13, lineHeight: 1.5 }}>
        12 documents · last touched Tuesday. Narrative carried over from the March update.
      </div>
    </Card>
  </div>
);

export const RuleVariant = () => (
  <div style={{ width: 360 }}>
    <Card variant="rule">
      <div className="font-head text-ink" style={{ fontSize: 15, marginBottom: 4 }}>
        Reading list
      </div>
      <div className="text-ink3" style={{ fontSize: 13, lineHeight: 1.5 }}>
        Set like a page — a top hairline and breathing room instead of a box.
      </div>
    </Card>
  </div>
);
