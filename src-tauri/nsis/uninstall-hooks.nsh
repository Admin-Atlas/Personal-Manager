; SPDX-FileCopyrightText: 2026 Bobby Yu
; SPDX-License-Identifier: AGPL-3.0-or-later

; Tauri NSIS uninstaller hook (wired via bundle.windows.nsis.installerHooks in
; tauri.windows.conf.json). PM keeps all of its user data OUTSIDE the install
; directory — under %LOCALAPPDATA%\Personal Manager — precisely so an update never
; wipes it (see src-tauri/src/paths.rs). That same isolation means a normal uninstall
; would otherwise leave behind the multi-hundred-MB regenerable runtime.
;
; So, after the app's own files are removed, we delete that regenerable runtime folder under the
; data dir: the managed Python venv (including the optional t-SNE and photo-OCR stacks) and the
; Whisper speech model — exactly what the in-app Settings -> Storage tab frees, and nothing the
; user would miss (it re-downloads on next use). This is the counterpart to the app's "Remove PM
; data" flow.
;
; Separately, and on EVERY uninstall, we force-remove the bundled standalone interpreter at
; $INSTDIR\python. It ships as an installer resource, but we run it IN PLACE to build the venv, so
; CPython scatters __pycache__\*.pyc through its Lib at runtime — untracked files the stock
; uninstaller never recorded and its RMDir can't clear, which would otherwise strand
; $INSTDIR\python\Lib (and, with it, the whole install dir) behind.
;
; Deliberately LEFT untouched on a NORMAL uninstall, so uninstall -> reinstall keeps everything:
;   * the Markdown vault and the encrypted database (the real user data),
;   * OS-keychain secrets (DB key, API keys, OAuth tokens),
;   * browser-side local storage (UI preferences, in the WebView2 folder).
;
; EXCEPTION - a full "remove PM completely" wipe inside the app (Settings -> Data &
; Security, behind several confirmations) deletes the user data itself, then drops a
; marker file in the WebView2 folder and launches this uninstaller. When that marker is
; present we ALSO purge the two folders the running app couldn't remove itself: its data
; dir (as a backstop, in case a stray handle blocked the app's own delete) and the in-use
; WebView2 folder. The marker lives OUTSIDE the data dir precisely so it survives the app
; clearing that dir; the app also clears any stale marker on a normal boot, so a cancelled
; full-uninstall can never make a later ordinary uninstall purge a still-wanted install.
;
; $LOCALAPPDATA resolves to the uninstalling user's local AppData (PM installs per-user),
; matching where the app itself resolves its data + WebView2 dirs. The identifier
; "org.itsatlas.pm" is fixed (renaming it orphans the keychain; see src-tauri/src/paths.rs).

!macro NSIS_HOOK_POSTUNINSTALL
  ; Always clear the bundled interpreter (see header): RMDir /r deletes the runtime-written .pyc the
  ; stock uninstaller can't, then drop the now-empty install dir (no-op if it isn't empty / is gone).
  RMDir /r "$INSTDIR\python"
  RMDir "$INSTDIR"

  IfFileExists "$LOCALAPPDATA\org.itsatlas.pm\.pm-uninstall-purge" pm_purge_all pm_keep_data
  pm_purge_all:
    RMDir /r "$LOCALAPPDATA\Personal Manager"
    RMDir /r "$LOCALAPPDATA\org.itsatlas.pm"
    Goto pm_purge_done
  pm_keep_data:
    RMDir /r "$LOCALAPPDATA\Personal Manager\runtime"
  pm_purge_done:
!macroend
