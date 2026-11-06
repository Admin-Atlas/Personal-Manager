// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Loaded by the probe as `<script type="module">` from the mod's own folder: whether a module script
// runs in the sealed frame. It is fetched in CORS mode from an opaque origin, and the scheme sends no
// Access-Control-Allow-Origin, so this measures whether a mod can ship Vite's default (module) build.
self.__pmModuleRan = true;
