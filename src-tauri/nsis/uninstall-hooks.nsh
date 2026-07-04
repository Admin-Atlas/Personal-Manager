; SPDX-FileCopyrightText: 2026 Bobby Yu
; SPDX-License-Identifier: AGPL-3.0-or-later

; Tauri NSIS uninstaller hook (wired via bundle.windows.nsis.installerHooks in
; tauri.windows.conf.json). PM keeps all of its user data OUTSIDE the install
; directory — under %LOCALAPPDATA%\Personal Manager — precisely so an update never
; wipes it (see src-tauri/src/paths.rs). That same isolation means a normal uninstall
; would otherwise leave behind the multi-hundred-MB regenerable runtime.
;
; So, after the app's own files are removed, we delete ONLY that runtime folder: the
; managed Python venv (including the optional t-SNE and photo-OCR stacks), the Whisper
; speech model, and the bundled standalone interpreter — exactly what the in-app
; Settings -> Storage tab frees, and nothing the user would miss (it re-downloads on
; next use). This is the counterpart to the app's "Remove PM data" flow.
;
; Deliberately LEFT untouched, so uninstall -> reinstall keeps everything:
;   * the Markdown vault and the encrypted database (the real user data),
;   * OS-keychain secrets (DB key, API keys, OAuth tokens),
;   * browser-side local storage (UI preferences).
; A full "remove PM from this machine" wipe of those is offered inside the app, behind
; several confirmations, rather than silently on uninstall.
;
; $LOCALAPPDATA resolves to the uninstalling user's local AppData (PM installs
; per-user), matching where the app itself resolves its data dir.

!macro NSIS_HOOK_POSTUNINSTALL
  RMDir /r "$LOCALAPPDATA\Personal Manager\runtime"
!macroend
