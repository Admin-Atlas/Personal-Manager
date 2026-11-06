// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `pmmod` scheme's state and its serve decision (Mods spec §1.4): which mods are installed and
//! exactly which of their files may be served, the one-time entry URLs ModFrame loads, and the frame
//! sessions the bridge calls under.
//!
//! **Entry tickets.** `mod_frame_open` mints a random single-use token bound to one mod's entry page
//! and a new session. The frame loads `<mod origin>/<id>/<entry page>?entry=<token>`. On WebKit the
//! navigation guard asks [`ModRuntime::is_pending_entry`] (which never consumes) before the frame
//! may navigate there; the handler then consumes the token on the first request that carries it,
//! and only that response is served as a document. Every other HTML, XHTML, XML or SVG response is
//! an attachment (headers.rs), so a mod's frame can't load a second page, another mod's page, or
//! its own page again: on WebView2, where the guard never sees frames, the token is the whole wall.
//! On macOS the attachment is no wall at all: wry's WKWebView response delegate shows any type
//! WebKit can display and never reads `Content-Disposition` (read from wry 0.57.1, not measured), so
//! there the guard alone keeps a non-entry page out. It can: a `pmmod` URL always parses (its host is
//! opaque), so the guard never fails open for one.
//!
//! **Races, and how each ends:**
//! - A token used twice: the first request takes it; the second gets an attachment.
//! - A token for mod A presented under mod B's path (or for another file): the token is burnt, A's
//!   session is closed, and B's file goes out as it would without a token.
//! - Two frames of one mod: each has its own token and session; whichever frame loads a URL takes
//!   that URL's session, and both are the same mod with the same grants.
//! - A session used after close, or a token whose session closed: closing drops the session and
//!   its unused token together, under one lock.
//! - A token or pending session older than [`ENTRY_TTL`]: expired, and swept on every open.
//! - An uninstall or reinstall: every ticket and session of that mod goes with it.
//! - Main's page loading again (a reload): every ticket and session goes ([`ModRuntime::close_all`]),
//!   since the page that held them is gone and its ModFrames never ran their cleanup.
//!
//! **Serving is bounded.** Each request reads and hashes its whole file, so at most
//! [`MAX_SERVING`] are read at once and [`MAX_WAITING`] more wait their turn ([`ServeGate`]); past
//! that a request is refused with a 503.
//!
//! The registry is in memory. The ledger (`ledger.rs`) persists installs in the `mods` table and
//! fills the registry from it whenever this instance opens a store as its writer.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::http::{Method, Request, Response, StatusCode, Uri};
use tauri::Url;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::engine::Platform;
use super::headers;
use super::ids::ModId;
use super::package::{self, VerifiedPackage};
use super::MAIN_LABEL;
use crate::pathguard;

/// How long a minted entry URL (and its session, until the frame loads) stays usable. ModFrame
/// loads it at once; a minute covers a slow first paint.
pub const ENTRY_TTL: Duration = Duration::from_secs(60);
/// Entry URLs waiting to load at once. More means something is minting without loading.
const MAX_PENDING: usize = 32;
const MAX_SESSIONS: usize = 64;
const TOKEN_BYTES: usize = 32;
const SESSION_BYTES: usize = 16;
/// Files read (and hashed) at once. Each can be up to a package entry's 16 MiB, so this bounds the
/// handler's memory and its share of tokio's blocking threads, which PM's own file and DB work use.
pub const MAX_SERVING: usize = 6;
/// Requests waiting for a read. A page loads a few dozen files; hundreds more is a mod flooding.
pub const MAX_WAITING: usize = 512;

/// The scheme's turnstile: [`MAX_SERVING`] reads at once, [`MAX_WAITING`] in line behind them.
pub struct ServeGate {
    reads: Arc<Semaphore>,
    waiting: AtomicUsize,
    max_waiting: usize,
}

/// One request's place in line, or its turn: it leaves the line when dropped, the turn included.
struct InLine<'a>(&'a AtomicUsize);

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ServeGate {
    pub fn new(max_serving: usize, max_waiting: usize) -> Self {
        Self {
            reads: Arc::new(Semaphore::new(max_serving)),
            waiting: AtomicUsize::new(0),
            max_waiting,
        }
    }

    /// Wait for a turn to read. `None` when the line is full (the request is refused) or the gate
    /// is closed (never, today). Holding the permit is holding the turn.
    pub async fn enter(&self) -> Option<OwnedSemaphorePermit> {
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= self.max_waiting {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        // Leaves the line once the turn comes, or if this future is dropped while waiting.
        let _in_line = InLine(&self.waiting);
        Arc::clone(&self.reads).acquire_owned().await.ok()
    }
}

/// Why a frame call was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    NotInstalled,
    TooManyFrames,
    /// The session was closed, never existed, or expired before its frame loaded.
    ClosedOrUnknown,
    /// The session's frame hasn't loaded its entry page yet.
    NotReady,
    UnknownMethod,
    Random,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotInstalled => "That mod isn't installed.",
            Self::TooManyFrames => "Too many mod frames are opening at once.",
            Self::ClosedOrUnknown => "That mod session has ended.",
            Self::NotReady => "That mod hasn't finished loading.",
            Self::UnknownMethod => "unknown method",
            Self::Random => "PM couldn't make a random token.",
        })
    }
}

/// What ModFrame gets: the URL its iframe loads once, and the session its bridge calls under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameOpened {
    pub entry_url: String,
    pub session: String,
}

/// One installed mod: its folder and exactly the files the verified package listed under `app/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledMod {
    id: ModId,
    /// Canonical, so containment compares like with like (pathguard's contract).
    root: PathBuf,
    entry: String,
    files: BTreeMap<String, ServedFile>,
}

/// A file as verified: served only while the bytes on disk still match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServedFile {
    pub sha256: [u8; 32],
    pub size: u64,
}

impl InstalledMod {
    /// A verified package whose files have been written under `root`.
    #[cfg_attr(not(test), allow(dead_code))] // the ledger's install, which the Mods tab calls (slice 6)
    pub fn from_package(pkg: &VerifiedPackage, root: &Path) -> std::io::Result<Self> {
        let files = pkg.files.iter().map(|(path, f)| {
            let served = ServedFile {
                sha256: f.sha256,
                size: f.bytes.len() as u64,
            };
            (path.clone(), served)
        });
        Self::from_ledger(
            pkg.manifest.id.clone(),
            root,
            &pkg.manifest.entry.page,
            files,
        )
    }

    /// A mod rebuilt from its `mods` row at startup (`ledger.rs`), from the row's own file list:
    /// the disk is never asked what to serve, and every serve re-checks the bytes against these
    /// hashes. Only `app/` files are served.
    pub fn from_ledger(
        id: ModId,
        root: &Path,
        entry: &str,
        files: impl IntoIterator<Item = (String, ServedFile)>,
    ) -> std::io::Result<Self> {
        let files = files
            .into_iter()
            .filter(|(path, _)| path.starts_with(headers::APP_DIR))
            .collect();
        Ok(Self {
            id,
            root: std::fs::canonicalize(root)?,
            entry: entry.to_owned(),
            files,
        })
    }
}

struct Ticket {
    session: String,
    mod_id: ModId,
    path: String,
    expires: Instant,
}

enum SessionState {
    /// Minted, its frame not loaded yet; its token is in `tickets`.
    Pending { token: String, expires: Instant },
    /// Its entry page was served.
    Live,
}

struct Session {
    mod_id: ModId,
    state: SessionState,
}

#[derive(Default)]
struct Inner {
    installed: HashMap<ModId, Arc<InstalledMod>>,
    tickets: HashMap<String, Ticket>,
    sessions: HashMap<String, Session>,
}

impl Inner {
    fn sweep(&mut self, now: Instant) {
        self.tickets.retain(|_, t| t.expires > now);
        self.sessions.retain(|_, s| match &s.state {
            SessionState::Pending { expires, .. } => *expires > now,
            SessionState::Live => true,
        });
    }

    fn drop_mod(&mut self, id: &ModId) {
        self.tickets.retain(|_, t| &t.mod_id != id);
        self.sessions.retain(|_, s| &s.mod_id != id);
    }

    /// Whether `token` is a live ticket for exactly this mod and file. Never consumes.
    fn ticket_matches(&self, target: &Target, token: &str, now: Instant) -> bool {
        self.tickets
            .get(token)
            .is_some_and(|t| t.expires > now && t.mod_id == target.id && t.path == target.path)
    }

    /// Consume `token`. True only when it was this mod's ticket for this file, unexpired, with its
    /// session still waiting; the session is then live. Any other use burns the token and closes
    /// its session.
    fn take_ticket(&mut self, target: &Target, token: &str, now: Instant) -> bool {
        let valid = self.ticket_matches(target, token, now);
        let Some(ticket) = self.tickets.remove(token) else {
            return false;
        };
        let waiting = matches!(
            self.sessions.get(&ticket.session),
            Some(Session { state: SessionState::Pending { token: t, .. }, .. }) if t == token
        );
        if valid && waiting {
            if let Some(session) = self.sessions.get_mut(&ticket.session) {
                session.state = SessionState::Live;
            }
            true
        } else {
            self.sessions.remove(&ticket.session);
            false
        }
    }
}

/// The scheme's state, managed by `setup()` for the life of the process.
pub struct ModRuntime {
    platform: Platform,
    /// PM's own page origins, which alone may frame a mod.
    frame_ancestors: Vec<String>,
    inner: Mutex<Inner>,
    gate: ServeGate,
    /// Held by every change to the mod folders and the `mods` rows (`ledger.rs`: install, update,
    /// uninstall, the startup load and sweep, the wipe), so the sweep can never see an install's
    /// folder between its rename and its row's commit.
    ledger: Mutex<()>,
}

impl ModRuntime {
    pub fn new(platform: Platform, frame_ancestors: Vec<String>) -> Self {
        Self {
            platform,
            frame_ancestors,
            inner: Mutex::new(Inner::default()),
            gate: ServeGate::new(MAX_SERVING, MAX_WAITING),
            ledger: Mutex::new(()),
        }
    }

    /// The ledger's lock. Synchronous work only: never held across an `.await`. A panic part way
    /// through a ledger change leaves the disk in a state the next sweep handles, so a poisoned
    /// lock is taken all the same.
    pub fn ledger_lock(&self) -> MutexGuard<'_, ()> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Make the registry exactly `mods` (the startup load, a vault switch, a closed store). A mod
    /// that stays exactly as it was keeps its frames; one that changed or left loses them.
    pub fn replace_all(&self, mods: Vec<InstalledMod>) {
        let mut inner = self.lock();
        let incoming: HashMap<ModId, InstalledMod> =
            mods.into_iter().map(|m| (m.id.clone(), m)).collect();
        let stale: Vec<ModId> = inner
            .installed
            .iter()
            .filter(|(id, m)| incoming.get(*id) != Some(m.as_ref()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stale {
            inner.installed.remove(id);
            inner.drop_mod(id);
        }
        for (id, m) in incoming {
            inner.installed.entry(id).or_insert_with(|| Arc::new(m));
        }
    }

    /// Where an installed mod is served from, for the ledger's tests.
    #[cfg(test)]
    pub fn installed_root(&self, id: &ModId) -> Option<PathBuf> {
        self.lock().installed.get(id).map(|m| m.root.clone())
    }

    /// The scheme handler's turnstile.
    pub fn gate(&self) -> &ServeGate {
        &self.gate
    }

    /// Every change under this lock is a handful of map inserts and removals that can't panic part
    /// way, so a poisoned lock still holds consistent state and is read through.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Install (or replace) a mod in the registry. A replacement ends every frame of the old one.
    #[cfg_attr(not(test), allow(dead_code))] // the ledger's install, which the Mods tab calls (slice 6)
    pub fn install(&self, installed: InstalledMod) {
        let mut inner = self.lock();
        inner.drop_mod(&installed.id);
        inner
            .installed
            .insert(installed.id.clone(), Arc::new(installed));
    }

    /// Remove a mod, its unused entry URLs and its sessions.
    #[cfg_attr(not(test), allow(dead_code))] // the ledger's uninstall and install (slice 6 calls them), the blacklist (slice 5)
    pub fn uninstall(&self, id: &ModId) {
        let mut inner = self.lock();
        inner.installed.remove(id);
        inner.drop_mod(id);
    }

    pub fn open_frame(&self, id: &ModId) -> Result<FrameOpened, FrameError> {
        self.open_frame_at(id, Instant::now())
    }

    fn open_frame_at(&self, id: &ModId, now: Instant) -> Result<FrameOpened, FrameError> {
        let token = random_hex(TOKEN_BYTES)?;
        let session = random_hex(SESSION_BYTES)?;
        let mut inner = self.lock();
        inner.sweep(now);
        let entry = inner
            .installed
            .get(id)
            .ok_or(FrameError::NotInstalled)?
            .entry
            .clone();
        if inner.tickets.len() >= MAX_PENDING || inner.sessions.len() >= MAX_SESSIONS {
            return Err(FrameError::TooManyFrames);
        }
        let expires = now + ENTRY_TTL;
        inner.tickets.insert(
            token.clone(),
            Ticket {
                session: session.clone(),
                mod_id: id.clone(),
                path: entry.clone(),
                expires,
            },
        );
        inner.sessions.insert(
            session.clone(),
            Session {
                mod_id: id.clone(),
                state: SessionState::Pending {
                    token: token.clone(),
                    expires,
                },
            },
        );
        Ok(FrameOpened {
            entry_url: format!(
                "{}/{id}/{entry}?{ENTRY_PARAM}={token}",
                self.platform.mod_origin()
            ),
            session,
        })
    }

    /// The navigation guard's question: is `url` an entry URL a frame is about to load? Read-only,
    /// so asking can't burn a token before the handler sees the request.
    pub fn is_pending_entry(&self, url: &Url) -> bool {
        self.is_pending_entry_at(url, Instant::now())
    }

    fn is_pending_entry_at(&self, url: &Url, now: Instant) -> bool {
        let Some(target) = target_of_url(url, self.platform.mod_origin()) else {
            return false;
        };
        let Some(token) = &target.token else {
            return false;
        };
        self.lock().ticket_matches(&target, token, now)
    }

    /// End every session and every unused entry URL: main's page is loading again, so every frame
    /// that held one is gone, and none of them ran its cleanup. Without this each would stay callable
    /// for the rest of the run and count against [`MAX_SESSIONS`] until PM restarted.
    pub fn close_all(&self) {
        let mut inner = self.lock();
        inner.tickets.clear();
        inner.sessions.clear();
    }

    /// End a session. Its unused entry URL dies with it. Ending one that has already ended is
    /// fine: a frame's unmount and its navigation-kill can both close it.
    pub fn close(&self, session: &str) {
        let mut inner = self.lock();
        if let Some(Session {
            state: SessionState::Pending { token, .. },
            ..
        }) = inner.sessions.remove(session)
        {
            inner.tickets.remove(&token);
        }
    }

    /// The mod a live session belongs to: PM's own record, never anything the frame said.
    pub fn session_mod(&self, session: &str) -> Result<ModId, FrameError> {
        self.session_mod_at(session, Instant::now())
    }

    fn session_mod_at(&self, session: &str, now: Instant) -> Result<ModId, FrameError> {
        let mut inner = self.lock();
        inner.sweep(now);
        match inner.sessions.get(session) {
            Some(Session {
                mod_id,
                state: SessionState::Live,
            }) => Ok(mod_id.clone()),
            Some(_) => Err(FrameError::NotReady),
            None => Err(FrameError::ClosedOrUnknown),
        }
    }

    /// A bridge call. The session is resolved to its mod first; the dispatcher (slice 4) then
    /// answers. Until it exists, every method is unknown.
    pub fn call(
        &self,
        session: &str,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, FrameError> {
        let _mod_id = self.session_mod(session)?;
        let _ = (method, args);
        Err(FrameError::UnknownMethod)
    }

    /// Answer one `pmmod` request from the webview labelled `label`.
    pub fn serve(&self, label: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        self.serve_at(label, req, Instant::now())
    }

    fn serve_at(&self, label: &str, req: &Request<Vec<u8>>, now: Instant) -> Response<Vec<u8>> {
        // v1 is main-window-only: no other window can load a mod file or spend an entry token.
        if label != MAIN_LABEL {
            return refusal(StatusCode::NOT_FOUND);
        }
        if req.method() != Method::GET {
            return refusal(StatusCode::METHOD_NOT_ALLOWED);
        }
        let Some(target) = target_of_request(req.uri()) else {
            return refusal(StatusCode::NOT_FOUND);
        };
        // Decide under the lock; read the file after it is released.
        let (installed, is_entry) = {
            let mut inner = self.lock();
            let is_entry = target
                .token
                .as_deref()
                .is_some_and(|t| inner.take_ticket(&target, t, now));
            (inner.installed.get(&target.id).cloned(), is_entry)
        };
        let Some(installed) = installed else {
            return refusal(StatusCode::NOT_FOUND);
        };
        let Some(want) = installed.files.get(&target.path) else {
            return refusal(StatusCode::NOT_FOUND);
        };
        let bytes = match read_verified(&installed.root, &target.path, want) {
            Ok(bytes) => bytes,
            Err(why) => {
                eprintln!(
                    "mods: refused to serve {}/{}: {why}",
                    installed.id, target.path
                );
                return refusal(StatusCode::NOT_FOUND);
            }
        };
        let ancestors: Vec<&str> = self.frame_ancestors.iter().map(String::as_str).collect();
        let Some(headers) = headers::file_headers(
            self.platform.mod_origin(),
            installed.id.as_str(),
            &target.path,
            &ancestors,
            is_entry,
        ) else {
            return refusal(StatusCode::INTERNAL_SERVER_ERROR);
        };
        let mut response = Response::builder().status(StatusCode::OK);
        for (name, value) in headers {
            response = response.header(name, value);
        }
        response
            .body(bytes)
            .unwrap_or_else(|_| refusal(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

/// An empty response with the refusal headers.
pub fn refusal(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers::refusal_headers() {
        response = response.header(name, value);
    }
    response.body(Vec::new()).unwrap_or_else(|_| {
        let mut bare = Response::new(Vec::new());
        *bare.status_mut() = status;
        bare
    })
}

/// The query parameter that carries an entry token.
const ENTRY_PARAM: &str = "entry";

/// A request for one mod file, maybe carrying an entry token.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    id: ModId,
    /// A package path under `app/`.
    path: String,
    token: Option<String>,
}

/// `/<id>/<package path>` and an optional `entry=<token>` query. The path is held to the package
/// path rules, so nothing percent-encoded, no dot segments, nothing outside `app/`; anything else
/// in the query is ignored (a bundler's cache-busting `?v=1`), and so is a malformed token.
fn parse_target(path: &str, query: Option<&str>) -> Option<Target> {
    let (id, file) = path.strip_prefix('/')?.split_once('/')?;
    let id = ModId::parse(id).ok()?;
    package::check_path(file).ok()?;
    if !file.starts_with(headers::APP_DIR) {
        return None;
    }
    let token = query
        .and_then(|q| q.strip_prefix(ENTRY_PARAM)?.strip_prefix('='))
        .filter(|t| is_token(t))
        .map(str::to_owned);
    Some(Target {
        id,
        path: file.to_owned(),
        token,
    })
}

/// The handler's URI. wry hands every engine's request over as `pmmod://localhost/…` (on WebView2 it
/// turns `http://pmmod.localhost/…` back into that before the handler sees it), so only that
/// authority is accepted: no other host, no port, no user info.
fn target_of_request(uri: &Uri) -> Option<Target> {
    if uri.scheme_str() != Some(super::SCHEME) || uri.authority()?.as_str() != "localhost" {
        return None;
    }
    parse_target(uri.path(), uri.query())
}

/// The guard's URL, as the engine sees it: under this platform's mod origin exactly.
fn target_of_url(url: &Url, mod_origin: &str) -> Option<Target> {
    let host = url.host_str()?;
    let plain = url.port().is_none() && url.username().is_empty() && url.password().is_none();
    if !plain || format!("{}://{host}", url.scheme()) != mod_origin {
        return None;
    }
    parse_target(url.path(), url.query())
}

fn is_token(s: &str) -> bool {
    s.len() == TOKEN_BYTES * 2 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn random_hex(bytes: usize) -> Result<String, FrameError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|_| FrameError::Random)?;
    Ok(hex::encode(buf))
}

/// The file under `root`, only while it is still inside `root` and still the verified bytes. A
/// file changed or swapped for a link after install is refused, not served.
fn read_verified(root: &Path, path: &str, want: &ServedFile) -> Result<Vec<u8>, &'static str> {
    let mut full = root.to_path_buf();
    full.extend(path.split('/'));
    let canonical = std::fs::canonicalize(&full).map_err(|_| "it is missing")?;
    if !pathguard::within_root(&canonical, root) {
        return Err("it resolves outside the mod's folder");
    }
    let file = File::open(&canonical).map_err(|_| "it can't be opened")?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err("it isn't a file");
    }
    let mut bytes = Vec::new();
    file.take(want.size + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "it can't be read")?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes.len() as u64 != want.size || hash != want.sha256 {
        return Err("it changed after it was verified");
    }
    Ok(bytes)
}

/// The origins that may frame a mod: PM's own page, and in a dev build the dev server that serves
/// it (`build.devUrl`).
pub fn frame_ancestors(platform: Platform, dev_url: Option<&Url>) -> Vec<String> {
    let mut out = vec![platform.app_origin().to_owned()];
    if let Some(url) = dev_url {
        if let Some(host) = url.host_str() {
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            out.push(format!("{}://{host}{port}", url.scheme()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "tauri://localhost";

    struct Setup {
        runtime: ModRuntime,
        _dir: tempfile::TempDir,
    }

    /// A runtime with `atlas.map` (and `acme.tool`) installed from real files.
    fn setup(platform: Platform) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let runtime = ModRuntime::new(platform, vec![ORIGIN.to_owned()]);
        for (id, files) in [
            (
                "atlas.map",
                vec![
                    ("app/index.html", b"<!doctype html><p>map".as_slice()),
                    ("app/main.js", b"console.log(1)".as_slice()),
                    ("app/icon.svg", b"<svg/>".as_slice()),
                    ("app/other.html", b"<p>other".as_slice()),
                    ("app/data.bin", b"\x00\x01".as_slice()),
                ],
            ),
            ("acme.tool", vec![("app/index.html", b"<p>acme".as_slice())]),
        ] {
            let root = dir.path().join(id);
            let mut served = BTreeMap::new();
            for (path, bytes) in files {
                let full = root.join(path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, bytes).unwrap();
                served.insert(
                    path.to_owned(),
                    ServedFile {
                        sha256: Sha256::digest(bytes).into(),
                        size: bytes.len() as u64,
                    },
                );
            }
            runtime.install(InstalledMod {
                id: ModId::parse(id).unwrap(),
                root: std::fs::canonicalize(&root).unwrap(),
                entry: "app/index.html".into(),
                files: served,
            });
        }
        Setup { runtime, _dir: dir }
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    /// The URI the handler sees for an entry URL (wry's form on every engine).
    fn handler_uri(entry_url: &str) -> String {
        entry_url.replacen("http://pmmod.localhost", "pmmod://localhost", 1)
    }

    fn get(rt: &ModRuntime, label: &str, uri: &str) -> Response<Vec<u8>> {
        let req = Request::builder().uri(uri).body(Vec::new()).unwrap();
        rt.serve(label, &req)
    }

    fn header<'a>(r: &'a Response<Vec<u8>>, name: &str) -> Option<&'a str> {
        r.headers().get(name).and_then(|v| v.to_str().ok())
    }

    fn is_document(r: &Response<Vec<u8>>) -> bool {
        r.status() == StatusCode::OK
            && header(r, "Content-Type") == Some("text/html; charset=utf-8")
            && header(r, "Content-Disposition").is_none()
    }

    fn is_attachment(r: &Response<Vec<u8>>) -> bool {
        r.status() == StatusCode::OK && header(r, "Content-Disposition") == Some("attachment")
    }

    #[test]
    fn an_entry_url_renders_once_then_its_session_is_live() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(opened
            .entry_url
            .starts_with("pmmod://localhost/atlas.map/app/index.html?entry="));
        assert_eq!(rt.session_mod(&opened.session), Err(FrameError::NotReady));
        let url = Url::parse(&opened.entry_url).unwrap();
        // The guard may ask as often as it likes: asking consumes nothing.
        assert!(rt.is_pending_entry(&url));
        assert!(rt.is_pending_entry(&url));

        let first = get(rt, "main", &opened.entry_url);
        assert!(is_document(&first));
        assert_eq!(first.body(), b"<!doctype html><p>map");
        assert_eq!(rt.session_mod(&opened.session), Ok(id("atlas.map")));
        // Used: the guard says no, and the same URL again is an attachment.
        assert!(!rt.is_pending_entry(&url));
        let second = get(rt, "main", &opened.entry_url);
        assert!(is_attachment(&second));
        // The session survives that: it was the frame's own load that went live.
        assert_eq!(rt.session_mod(&opened.session), Ok(id("atlas.map")));
    }

    #[test]
    fn subresources_are_served_with_the_mods_headers_and_documents_never_without_a_token() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let js = get(rt, "main", "pmmod://localhost/atlas.map/app/main.js");
        assert_eq!(js.status(), StatusCode::OK);
        assert_eq!(js.body(), b"console.log(1)");
        assert_eq!(
            header(&js, "Content-Type"),
            Some("text/javascript; charset=utf-8")
        );
        let csp = header(&js, "Content-Security-Policy").unwrap();
        assert!(csp.starts_with(headers::FIXED_DIRECTIVES));
        assert!(csp.contains("script-src pmmod://localhost/atlas.map/app/;"));
        assert!(csp.ends_with("frame-ancestors tauri://localhost"));
        for (name, value) in [
            ("X-Content-Type-Options", "nosniff"),
            ("X-DNS-Prefetch-Control", "off"),
            ("Cache-Control", "no-store"),
            ("Permissions-Policy", headers::PERMISSIONS_POLICY),
        ] {
            assert_eq!(header(&js, name), Some(value), "{name}");
        }
        assert_eq!(header(&js, "Content-Disposition"), None);
        // A query (a bundler's cache-buster) changes nothing.
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js?v=3").body(),
            b"console.log(1)"
        );
        for uri in [
            "pmmod://localhost/atlas.map/app/index.html",
            "pmmod://localhost/atlas.map/app/other.html",
            "pmmod://localhost/atlas.map/app/icon.svg",
            "pmmod://localhost/atlas.map/app/data.bin",
            &format!(
                "pmmod://localhost/atlas.map/app/index.html?entry={}",
                "0".repeat(64)
            ),
        ] {
            assert!(is_attachment(&get(rt, "main", uri)), "{uri}");
        }
    }

    #[test]
    fn only_the_main_window_and_only_get() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        for label in ["calendar", "briefing", "", "Main"] {
            let r = get(rt, label, &opened.entry_url);
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{label}");
            assert!(r.body().is_empty());
        }
        // Another window's request didn't spend the token.
        let post = Request::builder()
            .method(Method::POST)
            .uri(opened.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            rt.serve("main", &post).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert!(is_document(&get(rt, "main", &opened.entry_url)));
    }

    #[test]
    fn only_listed_app_files_under_the_mods_own_path_are_served() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        for uri in [
            // Not listed, or not a mod.
            "pmmod://localhost/atlas.map/app/missing.js",
            "pmmod://localhost/atlas.nope/app/index.html",
            "pmmod://localhost/atlas.map/manifest.json",
            "pmmod://localhost/atlas.map/LICENCE.txt",
            "pmmod://localhost/atlas.map/",
            "pmmod://localhost/atlas.map",
            "pmmod://localhost/",
            // Malformed or escaping.
            "pmmod://localhost/atlas.map/app/../../acme.tool/app/index.html",
            "pmmod://localhost/atlas.map/app/%2e%2e/x",
            "pmmod://localhost/atlas.map/app//main.js",
            "pmmod://localhost/Atlas.Map/app/main.js",
            "pmmod://localhost/atlas.map/APP/main.js",
            "pmmod://localhost/atlas.map/app/main.js%00",
            // Another authority or scheme.
            "pmmod://evil/atlas.map/app/main.js",
            "pmmod://localhost:8080/atlas.map/app/main.js",
            "pmmod://user@localhost/atlas.map/app/main.js",
            "http://pmmod.localhost/atlas.map/app/main.js",
            "tauri://localhost/atlas.map/app/main.js",
        ] {
            let r = get(rt, "main", uri);
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(r.body().is_empty(), "{uri}");
            assert!(header(&r, "Content-Security-Policy")
                .unwrap()
                .starts_with("sandbox;"));
        }
    }

    #[test]
    fn a_token_for_one_mod_under_another_mods_path_is_burnt() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let a = rt.open_frame(&id("atlas.map")).unwrap();
        let token = a.entry_url.rsplit_once('=').unwrap().1;
        let under_b = format!("pmmod://localhost/acme.tool/app/index.html?entry={token}");
        assert!(!rt.is_pending_entry(&Url::parse(&under_b).unwrap()));
        assert!(is_attachment(&get(rt, "main", &under_b)));
        // A's own URL no longer works, and its session is gone.
        assert!(!rt.is_pending_entry(&Url::parse(&a.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &a.entry_url)));
        assert_eq!(rt.session_mod(&a.session), Err(FrameError::ClosedOrUnknown));
    }

    #[test]
    fn a_token_for_the_entry_on_another_file_is_burnt() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let a = rt.open_frame(&id("atlas.map")).unwrap();
        let token = a.entry_url.rsplit_once('=').unwrap().1;
        let other = format!("pmmod://localhost/atlas.map/app/other.html?entry={token}");
        assert!(!rt.is_pending_entry(&Url::parse(&other).unwrap()));
        assert!(is_attachment(&get(rt, "main", &other)));
        assert!(is_attachment(&get(rt, "main", &a.entry_url)));
        assert_eq!(rt.session_mod(&a.session), Err(FrameError::ClosedOrUnknown));
    }

    #[test]
    fn two_frames_have_their_own_tokens_and_sessions() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let one = rt.open_frame(&id("atlas.map")).unwrap();
        let two = rt.open_frame(&id("atlas.map")).unwrap();
        assert_ne!(one.entry_url, two.entry_url);
        assert_ne!(one.session, two.session);
        assert!(is_document(&get(rt, "main", &two.entry_url)));
        assert_eq!(rt.session_mod(&one.session), Err(FrameError::NotReady));
        assert_eq!(rt.session_mod(&two.session), Ok(id("atlas.map")));
        assert!(is_document(&get(rt, "main", &one.entry_url)));
        assert_eq!(rt.session_mod(&one.session), Ok(id("atlas.map")));
        // Closing one leaves the other.
        rt.close(&one.session);
        assert_eq!(
            rt.session_mod(&one.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(rt.session_mod(&two.session), Ok(id("atlas.map")));
    }

    #[test]
    fn a_closed_session_is_dead_and_takes_its_unused_url_with_it() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        rt.close(&opened.session);
        rt.close(&opened.session); // twice is fine
        assert!(!rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &opened.entry_url)));
        assert_eq!(
            rt.session_mod(&opened.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            rt.call(&opened.session, "projects.list", &serde_json::Value::Null),
            Err(FrameError::ClosedOrUnknown)
        );
        // Closed after loading, too.
        let live = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &live.entry_url)));
        rt.close(&live.session);
        assert_eq!(
            rt.session_mod(&live.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            rt.session_mod("not-a-session"),
            Err(FrameError::ClosedOrUnknown)
        );
    }

    #[test]
    fn a_call_resolves_its_session_first_and_every_method_is_unknown_for_now() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        let args = serde_json::json!({});
        assert_eq!(
            rt.call(&opened.session, "projects.list", &args),
            Err(FrameError::NotReady)
        );
        assert!(is_document(&get(rt, "main", &opened.entry_url)));
        assert_eq!(
            rt.call(&opened.session, "projects.list", &args),
            Err(FrameError::UnknownMethod)
        );
    }

    #[test]
    fn an_unused_entry_url_and_its_session_expire() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let t0 = Instant::now();
        let opened = rt.open_frame_at(&id("atlas.map"), t0).unwrap();
        let url = Url::parse(&opened.entry_url).unwrap();
        let late = t0 + ENTRY_TTL;
        assert!(rt.is_pending_entry_at(&url, t0 + ENTRY_TTL - Duration::from_secs(1)));
        assert!(!rt.is_pending_entry_at(&url, late));
        let req = Request::builder()
            .uri(opened.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert!(is_attachment(&rt.serve_at("main", &req, late)));
        assert_eq!(
            rt.session_mod_at(&opened.session, late),
            Err(FrameError::ClosedOrUnknown)
        );
        // A live session doesn't expire.
        let live = rt.open_frame_at(&id("atlas.map"), t0).unwrap();
        let req = Request::builder()
            .uri(live.entry_url.as_str())
            .body(Vec::new())
            .unwrap();
        assert!(is_document(&rt.serve_at("main", &req, t0)));
        assert_eq!(
            rt.session_mod_at(&live.session, t0 + ENTRY_TTL * 10),
            Ok(id("atlas.map"))
        );
    }

    #[test]
    fn opening_is_bounded_and_needs_an_installed_mod() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        assert_eq!(
            rt.open_frame(&id("atlas.nope")),
            Err(FrameError::NotInstalled)
        );
        let t0 = Instant::now();
        for _ in 0..MAX_PENDING {
            rt.open_frame_at(&id("atlas.map"), t0).unwrap();
        }
        assert_eq!(
            rt.open_frame_at(&id("atlas.map"), t0),
            Err(FrameError::TooManyFrames)
        );
        // Expired ones are swept, which makes room.
        assert!(rt.open_frame_at(&id("atlas.map"), t0 + ENTRY_TTL).is_ok());
    }

    #[test]
    fn uninstalling_or_reinstalling_ends_every_frame_of_that_mod() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let live = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &live.entry_url)));
        let pending = rt.open_frame(&id("atlas.map")).unwrap();
        let other = rt.open_frame(&id("acme.tool")).unwrap();
        let again = rt.lock().installed[&id("atlas.map")].as_ref().clone();
        rt.install(again);
        assert_eq!(
            rt.session_mod(&live.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert!(!rt.is_pending_entry(&Url::parse(&pending.entry_url).unwrap()));
        assert!(rt.is_pending_entry(&Url::parse(&other.entry_url).unwrap()));
        rt.uninstall(&id("acme.tool"));
        assert_eq!(
            rt.session_mod(&other.session),
            Err(FrameError::ClosedOrUnknown)
        );
        assert_eq!(
            get(rt, "main", "pmmod://localhost/acme.tool/app/index.html").status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_file_changed_or_swapped_after_install_is_not_served() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let root = rt.lock().installed[&id("atlas.map")].root.clone();
        std::fs::write(root.join("app/main.js"), b"console.log(2)").unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::write(root.join("app/main.js"), b"console.log(1)!").unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_file(root.join("app/main.js")).unwrap();
        assert_eq!(
            get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
            StatusCode::NOT_FOUND
        );
        // A link out of the folder, even to identical bytes, is refused by containment.
        #[cfg(unix)]
        {
            let outside = root.parent().unwrap().join("outside.js");
            std::fs::write(&outside, b"console.log(1)").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("app/main.js")).unwrap();
            assert_eq!(
                get(rt, "main", "pmmod://localhost/atlas.map/app/main.js").status(),
                StatusCode::NOT_FOUND
            );
        }
    }

    #[test]
    fn on_webview2_the_entry_url_is_http_pmmod_localhost_and_the_handler_sees_wrys_form() {
        let s = setup(Platform::Windows);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(opened
            .entry_url
            .starts_with("http://pmmod.localhost/atlas.map/app/index.html?entry="));
        // The guard is never asked on WebView2, but if it were, only this origin would match.
        assert!(rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
        assert!(!rt.is_pending_entry(&Url::parse(&handler_uri(&opened.entry_url)).unwrap()));
        let r = get(rt, "main", &handler_uri(&opened.entry_url));
        assert!(is_document(&r));
        assert!(header(&r, "Content-Security-Policy")
            .unwrap()
            .contains("script-src http://pmmod.localhost/atlas.map/app/;"));
    }

    #[test]
    fn the_guard_matches_only_this_platforms_origin_and_a_well_formed_token() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let opened = rt.open_frame(&id("atlas.map")).unwrap();
        let token = opened.entry_url.rsplit_once('=').unwrap().1.to_owned();
        for url in [
            format!("http://pmmod.localhost/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://localhost:1/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://x@localhost/atlas.map/app/index.html?entry={token}"),
            format!("pmmod://localhost/atlas.map/app/index.html?entry={token}&x=1"),
            format!("pmmod://localhost/atlas.map/app/index.html?x=1&entry={token}"),
            format!(
                "pmmod://localhost/atlas.map/app/index.html?entry={}",
                token.to_uppercase()
            ),
            format!("pmmod://localhost/atlas.map/app/index.html#entry={token}"),
            "pmmod://localhost/atlas.map/app/index.html".to_owned(),
        ] {
            assert!(!rt.is_pending_entry(&Url::parse(&url).unwrap()), "{url}");
        }
        assert!(rt.is_pending_entry(&Url::parse(&opened.entry_url).unwrap()));
    }

    #[test]
    fn targets_parse_strictly() {
        let t = parse_target("/atlas.map/app/x/main.js", Some("entry=ab")).unwrap();
        assert_eq!(t.id, id("atlas.map"));
        assert_eq!(t.path, "app/x/main.js");
        assert_eq!(t.token, None, "a short token is no token");
        for (path, query) in [
            ("atlas.map/app/x.js", None),
            ("/atlas.map", None),
            ("/atlas.map/", None),
            ("/atlas.map/app/", None),
            ("/atlas.map/app/./x.js", None),
            ("/atlas..map/app/x.js", None),
            ("/con.map/app/x.js", None),
            ("/atlas.map/app/con.js", None),
            ("/atlas.map/app/x.js/", None),
        ] {
            assert_eq!(parse_target(path, query), None, "{path}");
        }
    }

    #[test]
    fn frame_ancestors_are_pms_page_plus_the_dev_server_in_dev() {
        assert_eq!(
            frame_ancestors(Platform::Linux, None),
            ["tauri://localhost"]
        );
        let dev = Url::parse("http://localhost:1420/").unwrap();
        assert_eq!(
            frame_ancestors(Platform::Windows, Some(&dev)),
            ["http://tauri.localhost", "http://localhost:1420"]
        );
        // Every ancestor is one the header builder accepts.
        let all = frame_ancestors(Platform::MacOs, Some(&dev));
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        assert!(headers::csp("pmmod://localhost", "a.b", &refs).is_some());
    }

    #[test]
    fn a_reload_of_mains_page_ends_every_session_and_frees_the_cap() {
        let s = setup(Platform::Linux);
        let rt = &s.runtime;
        let mut live = Vec::new();
        for _ in 0..MAX_SESSIONS {
            let opened = rt.open_frame(&id("atlas.map")).unwrap();
            assert!(is_document(&get(rt, "main", &opened.entry_url)));
            live.push(opened);
        }
        // Every frame leaked by a reload would sit here for the rest of the run.
        assert_eq!(
            rt.open_frame(&id("atlas.map")),
            Err(FrameError::TooManyFrames)
        );
        let pending = rt.open_frame(&id("acme.tool"));
        assert_eq!(pending, Err(FrameError::TooManyFrames));
        rt.close_all();
        for opened in &live {
            assert_eq!(
                rt.call(&opened.session, "projects.list", &serde_json::Value::Null),
                Err(FrameError::ClosedOrUnknown)
            );
        }
        // An entry URL minted before the reload is dead too.
        let before = rt.open_frame(&id("atlas.map")).unwrap();
        rt.close_all();
        assert!(!rt.is_pending_entry(&Url::parse(&before.entry_url).unwrap()));
        assert!(is_attachment(&get(rt, "main", &before.entry_url)));
        // And the cap is free again.
        let after = rt.open_frame(&id("atlas.map")).unwrap();
        assert!(is_document(&get(rt, "main", &after.entry_url)));
        assert_eq!(rt.session_mod(&after.session), Ok(id("atlas.map")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_burst_of_requests_is_read_a_few_at_a_time_and_a_flood_is_refused() {
        let gate = Arc::new(ServeGate::new(3, 40));
        let reading = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..40 {
            let (gate, reading, most) = (gate.clone(), reading.clone(), most.clone());
            tasks.push(tokio::spawn(async move {
                let Some(_turn) = gate.enter().await else {
                    return false;
                };
                let now = reading.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                reading.fetch_sub(1, Ordering::SeqCst);
                true
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap(), "a request within the line was refused");
        }
        let most = most.load(Ordering::SeqCst);
        assert!((1..=3).contains(&most), "{most} read at once");
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 0);

        // A full line refuses the next request at once, and a place frees when one leaves.
        let gate = ServeGate::new(1, 2);
        let turn = gate.enter().await.unwrap();
        // Poll each once so both are in line behind the held turn.
        let mut first = Box::pin(gate.enter());
        let mut second = Box::pin(gate.enter());
        assert!(futures_poll_once(&mut first).await.is_none());
        assert!(futures_poll_once(&mut second).await.is_none());
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 2);
        assert!(gate.enter().await.is_none(), "a third in line");
        // One gives up its place (its future dropped): the line has room again.
        drop(second);
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 1);
        drop(turn);
        assert!(first.await.is_some());
        assert_eq!(gate.waiting.load(Ordering::SeqCst), 0);
    }

    /// Poll a future once: `Some` with its output if it was ready, `None` if it is still waiting.
    async fn futures_poll_once<F: std::future::Future + Unpin>(f: &mut F) -> Option<F::Output> {
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match std::pin::Pin::new(&mut *f).poll(cx) {
                std::task::Poll::Ready(out) => Some(out),
                std::task::Poll::Pending => None,
            })
        })
        .await
    }

    #[test]
    fn tokens_and_sessions_are_random_and_hex() {
        let s = setup(Platform::Linux);
        let a = s.runtime.open_frame(&id("atlas.map")).unwrap();
        let b = s.runtime.open_frame(&id("atlas.map")).unwrap();
        let token = |o: &FrameOpened| o.entry_url.rsplit_once('=').unwrap().1.to_owned();
        assert!(is_token(&token(&a)) && is_token(&token(&b)));
        assert_ne!(token(&a), token(&b));
        assert_eq!(a.session.len(), SESSION_BYTES * 2);
        assert_ne!(a.session, b.session);
    }
}
