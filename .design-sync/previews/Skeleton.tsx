// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Skeleton } from "pm";

export const LoadingCard = () => (
  <div style={{ width: 360, display: "flex", flexDirection: "column", gap: 10 }}>
    <Skeleton style={{ height: 14, width: "55%" }} />
    <Skeleton style={{ height: 12, width: "100%" }} />
    <Skeleton style={{ height: 12, width: "85%" }} />
    <Skeleton style={{ height: 12, width: "70%" }} />
  </div>
);

export const Avatars = () => (
  <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
    <Skeleton style={{ height: 40, width: 40, borderRadius: 999 }} />
    <div style={{ display: "flex", flexDirection: "column", gap: 8, flex: 1 }}>
      <Skeleton style={{ height: 12, width: 140 }} />
      <Skeleton style={{ height: 10, width: 90 }} />
    </div>
  </div>
);
