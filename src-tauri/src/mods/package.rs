// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `.pmmod` verifier (Mods spec §1.2): a pure function from a file's bytes and the trusted
//! public keys to a [`VerifiedPackage`], or the reason it was refused.
//!
//! Nothing in the file is believed before the signature says so, and nothing the zip declares
//! about itself is believed at all:
//! - the zip's own structure first (one archive from byte 0, no duplicate or inconsistent headers,
//!   no overlapping entries), then every entry's name and type, before any data is read;
//! - the minisign signature over `pmmod/checksums.json`, by one of the trusted keys, prehashed only;
//!   its trusted comment (covered by the global signature, so authentic) names the id, version,
//!   tier, author, source repo, source commit and API range;
//! - the zip holds exactly the files `checksums.json` lists, plus the two `pmmod/` files, and each
//!   one's sha256 matches;
//! - every size cap is enforced on the bytes actually decompressed, stopping at the cap, never on a
//!   size the zip declares;
//! - the manifest's fields are typed and checked, and agree with the trusted comment.
//!
//! The package's files come back as the verified bytes, so the install path writes exactly what was
//! hashed: no second parse of the zip can disagree with this one.
//!
//! **Not here** (they need state this function doesn't have; spec §1.2's last sentence): the index
//! entry's agreement and the whole file's sha256 against it, the blacklist, "newer than installed"
//! (or a repair's pinned version and sha256), the publisher pin, and whether PM supports the API
//! range. The install path runs them on the [`VerifiedPackage`]. So does the check that an unsigned
//! file's id isn't a catalogue id; unsigned (third-party) files get their own entry point with the
//! dev-mode install.
//!
//! **The trusted comment's format** (the pm-mods sign job writes it with `minisign -S -t`): eight
//! `key:value` fields separated by tabs, in this order, each exactly once:
//! `pmmod:1`, `id:<id>`, `version:<semver>`, `tier:first|second`, `author:<text>`,
//! `sourceRepo:<owner/repo>`, `commit:<40 lowercase hex>`, `api:<min>-<target>`. The leading
//! `pmmod:1` keeps a signature made for one kind of file (an index, a key revocation) from passing
//! as a mod's. Every field but `pmmod` must equal the manifest's exactly, so the file itself vouches
//! for the commit it was built from, not only the signed index entry.
//!
//! **`checksums.json`:** `{"format":1,"files":[{"path":…,"sha256":…},…]}`, sorted by path, listing
//! every entry but the two `pmmod/` files.

// The install path (Mods PR-1, slice 3) is the verifier's caller; in this slice only its tests are.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Read};

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use zip::read::{ArchiveOffset, Config};
use zip::{CompressionMethod, ZipArchive};

use super::headers::APP_DIR;
use super::ids::{self, ModId, Tier};

pub const MANIFEST: &str = "manifest.json";
pub const LICENCE_FILE: &str = "LICENCE.txt";
pub const NOTICE_FILE: &str = "NOTICE.txt";
pub const CHECKSUMS: &str = "pmmod/checksums.json";
pub const SIGNATURE: &str = "pmmod/signature.minisig";
/// The only licence a mod may carry (spec §1.2; AGPL source is what keeps every mod reviewable).
pub const LICENCE: &str = "AGPL-3.0-or-later";

/// The permission ids API 1.0 defines (spec §1.5). The dispatcher's table becomes the source of
/// truth with the API (slice 4); a test there should pin this list to it.
pub const PERMISSIONS: &[&str] = &[
    "storage",
    "ui.tab",
    "notify",
    "projects.read",
    "tasks.read",
    "documents.meta",
    "documents.vectors",
    "documents.text",
    "calendar.read",
    "calendar.details",
    "tasks.write",
    "milestones.write",
    "documents.write",
    "ai",
    "internet",
    "links",
];

/// Headers a secret may never go in: hop-by-hop ones and those the request itself owns (spec §1.5).
const FORBIDDEN_SECRET_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "cookie",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Size and count caps, applied to what is actually decompressed.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The `.pmmod` file itself.
    pub file: usize,
    pub entries: usize,
    /// One decompressed entry.
    pub entry: usize,
    /// Every decompressed entry together. Also bounds the verified package's memory.
    pub total: usize,
}

impl Limits {
    /// A mod is a bundled web app: the Map's is well under a megabyte. These leave a lot of room for
    /// fonts and data while keeping a hostile file's cost bounded.
    pub const DEFAULT: Self = Self {
        file: 32 << 20,
        entries: 4096,
        entry: 16 << 20,
        total: 64 << 20,
    };
}

const MANIFEST_CAP: usize = 256 << 10;
const CHECKSUMS_CAP: usize = 1 << 20;
const SIGNATURE_CAP: usize = 4 << 10;
const PATH_CAP: usize = 200;
const COMPONENT_CAP: usize = 100;
const DEPTH_CAP: usize = 16;

/// Which rule refused a file. Tests match on it; people read [`PackageError`]'s text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooLarge,
    NotAZip,
    ZipLayout,
    TooManyEntries,
    EntryName,
    NotAFile,
    Encrypted,
    Compression,
    CaseCollision,
    EntryTooLarge,
    TotalTooLarge,
    Corrupt,
    Unsigned,
    Signature,
    UntrustedKey,
    BadSignature,
    TrustedComment,
    Checksums,
    Unlisted,
    Missing,
    ChecksumMismatch,
    Manifest,
    Mismatch,
    Licence,
    Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageError {
    pub refusal: Refusal,
    pub detail: String,
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "This mod file was refused: {}.", self.detail)
    }
}

impl std::error::Error for PackageError {}

fn refuse(refusal: Refusal, detail: impl Into<String>) -> PackageError {
    PackageError {
        refusal,
        detail: detail.into(),
    }
}

/// A name from the file, quoted and escaped so it can't fake the message around it.
fn quote(name: &str) -> String {
    let mut out: String = name.chars().take(80).collect();
    if out.len() < name.len() {
        out.push('…');
    }
    format!("{out:?}")
}

// ---- The result --------------------------------------------------------------------------------

/// A package every rule above passed.
#[derive(Debug, Clone)]
pub struct VerifiedPackage {
    pub manifest: Manifest,
    /// Every file `checksums.json` lists, keyed by package path, as the bytes that were hashed.
    pub files: BTreeMap<String, VerifiedFile>,
    /// Which of the trusted keys signed it.
    pub signer: usize,
    /// The signed trusted comment, verbatim.
    pub trusted_comment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFile {
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

/// The manifest, every field checked.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub id: ModId,
    pub name: String,
    pub description: String,
    /// A package path under `app/`, a raster or an SVG (shown only through `<img>`).
    pub icon: Option<String>,
    pub version: semver::Version,
    pub api: ApiRange,
    pub publisher: Publisher,
    pub permissions: Vec<Permission>,
    pub internet_hosts: Vec<String>,
    pub secrets: Vec<SecretSlot>,
    pub contributions: Contributions,
    pub data: DataDecl,
    pub entry: Entry,
    pub requires: Vec<Room>,
}

/// `apiVersion`: the API range a mod was built for, as `(major, minor)` pairs, `min <= target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiRange {
    pub min: (u16, u16),
    pub target: (u16, u16),
}

impl ApiRange {
    /// `<min>-<target>`, as the trusted comment writes it.
    pub fn comment_form(&self) -> String {
        format!(
            "{}.{}-{}.{}",
            self.min.0, self.min.1, self.target.0, self.target.1
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publisher {
    pub tier: Tier,
    pub author: String,
    pub source_repo: String,
    /// The source commit, 40 lowercase hex. The trusted comment carries it too and must equal it,
    /// so the signature vouches for it (Bobby's decision, 2026-11-05), as the index entry does.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permission {
    pub id: String,
    /// Shown at install.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSlot {
    pub name: String,
    /// The one host it goes to, one of `internetHosts`.
    pub host: String,
    /// The one header it goes in.
    pub header: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contributions {
    /// The tab's label.
    pub nav_tab: Option<String>,
    /// The overlay's label in the calendar's source list.
    pub calendar_overlay: Option<String>,
    pub help: Vec<HelpEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelpEntry {
    pub id: String,
    pub title: String,
    /// Markdown, rendered only through PM's `markdown.tsx`.
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DataDecl {
    #[serde(default)]
    pub keys: Vec<DataKey>,
    /// JSON paths into a key's value that hold project names (spec §4). The path grammar, with its
    /// recursive form, is PR-2's; here a path is only shape-checked.
    #[serde(default)]
    pub project_refs: Vec<ProjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataKey {
    pub key: String,
    pub class: DataClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataClass {
    /// The user's: kept on update, asked about on uninstall, in backups.
    Content,
    /// Rebuildable: dropped freely.
    Cache,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRef {
    pub key: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The frame's page, an `.html` under `app/`.
    pub page: String,
    /// Scripts under `app/` a page runs as Workers (built as blob Workers inside the frame).
    #[serde(default)]
    pub workers: Vec<String>,
}

/// Core rooms a mod needs, so it can park while one is switched off (spec §1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Room {
    Calendar,
    Localai,
}

// ---- The manifest as written -------------------------------------------------------------------

/// `deny_unknown_fields` everywhere and no `flatten`, so an unknown or repeated key is an error, not
/// a silently ignored or overwritten one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawManifest {
    id: String,
    name: String,
    description: String,
    #[serde(default, deserialize_with = "present")]
    icon: Option<String>,
    version: String,
    api_version: RawApi,
    publisher: RawPublisher,
    licence: String,
    #[serde(default)]
    permissions: Vec<Permission>,
    #[serde(default)]
    internet_hosts: Vec<String>,
    #[serde(default)]
    secrets: Vec<SecretSlot>,
    #[serde(default)]
    contributions: RawContributions,
    #[serde(default)]
    data: DataDecl,
    entry: Entry,
    #[serde(default)]
    requires: Vec<Room>,
    /// Reserved (spec §1.2): absent or exactly `{"frame": true}`, so a later engine is an additive
    /// value, never a format break.
    #[serde(default, deserialize_with = "present")]
    engines: Option<RawEngines>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApi {
    min: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPublisher {
    tier: String,
    author: String,
    source_repo: String,
    commit: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContributions {
    #[serde(rename = "nav.tab", default, deserialize_with = "present")]
    nav_tab: Option<RawLabel>,
    #[serde(rename = "calendar.overlay", default, deserialize_with = "present")]
    calendar_overlay: Option<RawLabel>,
    #[serde(default)]
    help: Vec<HelpEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabel {
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEngines {
    frame: bool,
}

/// An optional field that, when present, must hold a value: `null` is refused rather than read as
/// absent.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChecksums {
    format: u32,
    files: Vec<RawChecksum>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChecksum {
    path: String,
    sha256: String,
}

// ---- Verify ------------------------------------------------------------------------------------

/// Verify a signed `.pmmod` against `keys` (the embedded Atlas keys, slice 5). With no keys,
/// nothing verifies.
pub fn verify(bytes: &[u8], keys: &[PublicKey]) -> Result<VerifiedPackage, PackageError> {
    verify_with(bytes, keys, &Limits::DEFAULT)
}

/// [`verify`] under the given caps.
pub fn verify_with(
    bytes: &[u8],
    keys: &[PublicKey],
    limits: &Limits,
) -> Result<VerifiedPackage, PackageError> {
    if bytes.len() > limits.file {
        return Err(refuse(Refusal::TooLarge, "the file is too large"));
    }
    let mut reader = Archive::open(bytes, limits)?;
    let names = reader.check_entries()?;

    // The signature first: nothing in checksums.json is believed until it verifies.
    let signature = reader
        .read(SIGNATURE, SIGNATURE_CAP)?
        .ok_or_else(|| refuse(Refusal::Unsigned, "it isn't signed"))?;
    let checksums = reader
        .read(CHECKSUMS, CHECKSUMS_CAP)?
        .ok_or_else(|| refuse(Refusal::Missing, format!("it has no {CHECKSUMS}")))?;
    let (signer, trusted_comment) = verify_signature(&checksums, &signature, keys)?;
    let comment = TrustedComment::parse(&trusted_comment)?;
    let listed = parse_checksums(&checksums)?;

    // Exactly the listed files, plus the two that list and sign them.
    let mut expected: BTreeSet<&str> = listed.keys().map(String::as_str).collect();
    expected.insert(CHECKSUMS);
    expected.insert(SIGNATURE);
    let present: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    if let Some(extra) = present.difference(&expected).next() {
        return Err(refuse(
            Refusal::Unlisted,
            format!("it holds {} that its checksums don't list", quote(extra)),
        ));
    }
    if let Some(missing) = expected.difference(&present).next() {
        return Err(refuse(
            Refusal::Missing,
            format!(
                "its checksums list {}, which it doesn't hold",
                quote(missing)
            ),
        ));
    }

    let mut files = BTreeMap::new();
    for (path, want) in &listed {
        let cap = if path == MANIFEST {
            MANIFEST_CAP
        } else {
            limits.entry
        };
        let bytes = reader
            .read(path, cap)?
            .ok_or_else(|| refuse(Refusal::Missing, format!("{} is missing", quote(path))))?;
        let got: [u8; 32] = Sha256::digest(&bytes).into();
        if &got != want {
            return Err(refuse(
                Refusal::ChecksumMismatch,
                format!("{} isn't the file that was signed", quote(path)),
            ));
        }
        files.insert(path.clone(), VerifiedFile { sha256: got, bytes });
    }

    let manifest_bytes = &files
        .get(MANIFEST)
        .ok_or_else(|| refuse(Refusal::Missing, format!("it has no {MANIFEST}")))?
        .bytes;
    let manifest = parse_manifest(manifest_bytes, &files)?;
    comment.check_against(&manifest)?;
    ids::check_tier(&manifest.id, manifest.publisher.tier)
        .map_err(|e| refuse(Refusal::Id, e.to_string()))?;
    check_licence_text(&files)?;

    Ok(VerifiedPackage {
        manifest,
        files,
        signer,
        trusted_comment,
    })
}

/// The zip, opened strictly, with a running budget for what has been decompressed.
struct Archive<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    left: usize,
    limits: Limits,
}

impl<'a> Archive<'a> {
    fn open(bytes: &'a [u8], limits: &Limits) -> Result<Self, PackageError> {
        // `Known(0)`: one archive starting at the first byte, so nothing is hidden in front of it.
        let config = Config {
            archive_offset: ArchiveOffset::Known(0),
        };
        let mut zip = ZipArchive::with_config(config, Cursor::new(bytes))
            .map_err(|e| refuse(Refusal::NotAZip, format!("it isn't a readable zip ({e})")))?;
        if zip.len() > limits.entries {
            return Err(refuse(Refusal::TooManyEntries, "it holds too many files"));
        }
        if zip.has_duplicate_names() {
            return Err(refuse(Refusal::ZipLayout, "it names one file twice"));
        }
        // Each entry's local header agrees with the central directory, and no two entries share
        // bytes (the overlapping-entry zip bomb).
        zip.validate_header_consistency().map_err(|e| {
            refuse(
                Refusal::ZipLayout,
                format!("its zip headers don't agree ({e})"),
            )
        })?;
        Ok(Self {
            zip,
            left: limits.total,
            limits: *limits,
        })
    }

    /// Every entry's name and type, before any data is read. Returns the names.
    fn check_entries(&mut self) -> Result<Vec<String>, PackageError> {
        let mut names = Vec::with_capacity(self.zip.len());
        for index in 0..self.zip.len() {
            let entry = self
                .zip
                .by_index_data(index)
                .map_err(|e| refuse(Refusal::NotAZip, format!("it isn't a readable zip ({e})")))?;
            let raw = entry.name_raw();
            let name = std::str::from_utf8(raw).map_err(|_| {
                refuse(
                    Refusal::EntryName,
                    format!(
                        "{} isn't a plain file name",
                        quote(&String::from_utf8_lossy(raw))
                    ),
                )
            })?;
            if name.ends_with('/') || entry.is_dir() {
                return Err(refuse(
                    Refusal::NotAFile,
                    format!("{} is a folder entry", quote(name)),
                ));
            }
            // Regular files only: no symlinks, no devices, FIFOs or sockets.
            let special = entry
                .unix_mode()
                .is_some_and(|m| !matches!(m & 0o170000, 0 | 0o100000));
            if entry.is_symlink() || special {
                return Err(refuse(
                    Refusal::NotAFile,
                    format!("{} isn't a plain file", quote(name)),
                ));
            }
            check_path(name)?;
            if entry.encrypted() {
                return Err(refuse(
                    Refusal::Encrypted,
                    format!("{} is encrypted", quote(name)),
                ));
            }
            if !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                return Err(refuse(
                    Refusal::Compression,
                    format!("{} isn't stored or deflated", quote(name)),
                ));
            }
            names.push(name.to_owned());
        }
        check_case_collisions(&names)?;
        Ok(names)
    }

    /// Decompress one entry, at most `cap` bytes of it (and no more than the total budget left),
    /// reading one byte past the cap to tell "exactly at" from "over". `None` when it isn't there.
    fn read(&mut self, name: &str, cap: usize) -> Result<Option<Vec<u8>>, PackageError> {
        let Some(index) = self.zip.index_for_name(name) else {
            return Ok(None);
        };
        let cap = cap.min(self.limits.entry);
        let limit = cap.min(self.left);
        let corrupt = |e: &dyn fmt::Display| {
            refuse(
                Refusal::Corrupt,
                format!("{} couldn't be read ({e})", quote(name)),
            )
        };
        let mut file = self.zip.by_index(index).map_err(|e| corrupt(&e))?;
        let mut out = Vec::new();
        file.by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|e| corrupt(&e))?;
        if out.len() > limit {
            return Err(if limit < cap {
                refuse(Refusal::TotalTooLarge, "its files are too large together")
            } else {
                refuse(
                    Refusal::EntryTooLarge,
                    format!("{} is too large", quote(name)),
                )
            });
        }
        self.left -= out.len();
        Ok(Some(out))
    }
}

/// A package path: relative, `/`-separated, in a small ASCII set that needs no escaping in a URL
/// and means the same file on every filesystem. Refused: absolute paths, `..` and `.`, `:` (drive
/// letters, NTFS streams), backslashes, a trailing dot or space (Windows strips them), device names,
/// `~` (NTFS short names like `PROGRA~1` alias other files), and anything outside the package
/// layout (`manifest.json`, `LICENCE.txt`, `NOTICE.txt`, `app/…`, the two `pmmod/` files).
pub fn check_path(path: &str) -> Result<(), PackageError> {
    let bad = |why: &str| {
        refuse(
            Refusal::EntryName,
            format!("{} isn't allowed in a mod ({why})", quote(path)),
        )
    };
    if path.is_empty() {
        return Err(bad("empty name"));
    }
    if path.len() > PATH_CAP {
        return Err(bad("name too long"));
    }
    if path.starts_with('/') {
        return Err(bad("absolute path"));
    }
    if path.contains('\\') {
        return Err(bad("backslash"));
    }
    if path.contains(':') {
        return Err(bad("colon"));
    }
    let components: Vec<&str> = path.split('/').collect();
    if components.len() > DEPTH_CAP {
        return Err(bad("too deep"));
    }
    for c in &components {
        if c.is_empty() {
            return Err(bad("empty folder name"));
        }
        if *c == "." || *c == ".." {
            return Err(bad("dot segment"));
        }
        if c.len() > COMPONENT_CAP {
            return Err(bad("name too long"));
        }
        if c.ends_with('.') || c.ends_with(' ') {
            return Err(bad("trailing dot or space"));
        }
        if !c
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        {
            return Err(bad("character"));
        }
        if ids::is_device_name(c) {
            return Err(bad("device name"));
        }
    }
    let in_layout = matches!(
        path,
        MANIFEST | LICENCE_FILE | NOTICE_FILE | CHECKSUMS | SIGNATURE
    ) || path.starts_with(APP_DIR);
    if !in_layout {
        return Err(bad("outside the package layout"));
    }
    Ok(())
}

/// No two entries the same after ASCII case-folding (NTFS and APFS fold case), and no file whose
/// folded name is also a folder another entry sits in.
fn check_case_collisions(names: &[String]) -> Result<(), PackageError> {
    let mut folded: BTreeMap<String, &str> = BTreeMap::new();
    for name in names {
        if let Some(other) = folded.insert(name.to_ascii_lowercase(), name) {
            return Err(refuse(
                Refusal::CaseCollision,
                format!(
                    "{} and {} are the same file on Windows and macOS",
                    quote(other),
                    quote(name)
                ),
            ));
        }
    }
    for name in names {
        let lower = name.to_ascii_lowercase();
        for (i, _) in lower.match_indices('/') {
            if let Some(file) = folded.get(&lower[..i]) {
                return Err(refuse(
                    Refusal::CaseCollision,
                    format!("{} is a file and a folder", quote(file)),
                ));
            }
        }
    }
    Ok(())
}

/// Verify `signature` (a minisign signature file) over `message` with any of `keys`. Returns which
/// key and the trusted comment.
fn verify_signature(
    message: &[u8],
    signature: &[u8],
    keys: &[PublicKey],
) -> Result<(usize, String), PackageError> {
    let malformed = || refuse(Refusal::Signature, "its signature is malformed");
    let text = std::str::from_utf8(signature).map_err(|_| malformed())?;
    // minisign writes exactly four lines; anything after them would be ignored by the decoder, and
    // nothing in a signed file is ignored.
    if text.lines().count() != 4 {
        return Err(malformed());
    }
    let sig = Signature::decode(text).map_err(|_| malformed())?;
    let mut id_matched = false;
    for (index, key) in keys.iter().enumerate() {
        match key.verify(message, &sig, false) {
            Ok(()) => return Ok((index, sig.trusted_comment().to_owned())),
            Err(minisign_verify::Error::UnexpectedKeyId) => {}
            Err(minisign_verify::Error::UnexpectedAlgorithm) => {
                return Err(refuse(
                    Refusal::Signature,
                    "its signature uses minisign's legacy mode, which PM refuses",
                ));
            }
            Err(_) => id_matched = true,
        }
    }
    Err(if id_matched {
        refuse(Refusal::BadSignature, "its signature doesn't verify")
    } else {
        refuse(Refusal::UntrustedKey, "it isn't signed by a key PM trusts")
    })
}

/// The signed trusted comment's fields.
#[derive(Debug, PartialEq, Eq)]
struct TrustedComment {
    id: String,
    version: String,
    tier: Tier,
    author: String,
    source_repo: String,
    /// 40 lowercase hex, checked here as well as in the manifest.
    commit: String,
    api: String,
}

const COMMENT_KEYS: [&str; 8] = [
    "pmmod",
    "id",
    "version",
    "tier",
    "author",
    "sourceRepo",
    "commit",
    "api",
];

impl TrustedComment {
    fn parse(comment: &str) -> Result<Self, PackageError> {
        let bad = |why: &str| {
            refuse(
                Refusal::TrustedComment,
                format!("its signed description is malformed ({why})"),
            )
        };
        let fields: Vec<&str> = comment.split('\t').collect();
        if fields.len() != COMMENT_KEYS.len() {
            return Err(bad("wrong number of fields"));
        }
        let mut values = Vec::with_capacity(fields.len());
        for (field, key) in fields.iter().zip(COMMENT_KEYS) {
            let value = field
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(':'))
                .ok_or_else(|| bad(&format!("expected {key}")))?;
            if value.is_empty() || value.chars().any(char::is_control) {
                return Err(bad(&format!("{key} is empty or has a control character")));
            }
            values.push(value);
        }
        if values[0] != "1" {
            return Err(bad("not a mod's signature"));
        }
        let tier = match Tier::parse(values[3]) {
            Some(t @ (Tier::First | Tier::Second)) => t,
            _ => return Err(bad("tier")),
        };
        if !is_commit(values[6]) {
            return Err(bad("commit isn't 40 lowercase hex digits"));
        }
        Ok(Self {
            id: values[1].to_owned(),
            version: values[2].to_owned(),
            tier,
            author: values[4].to_owned(),
            source_repo: values[5].to_owned(),
            commit: values[6].to_owned(),
            api: values[7].to_owned(),
        })
    }

    /// Every field the comment carries equals the manifest's, exactly.
    fn check_against(&self, m: &Manifest) -> Result<(), PackageError> {
        let pairs = [
            ("id", self.id.as_str(), m.id.as_str().to_owned()),
            ("version", self.version.as_str(), m.version.to_string()),
            (
                "tier",
                self.tier.as_str(),
                m.publisher.tier.as_str().to_owned(),
            ),
            ("author", self.author.as_str(), m.publisher.author.clone()),
            (
                "source repo",
                self.source_repo.as_str(),
                m.publisher.source_repo.clone(),
            ),
            ("commit", self.commit.as_str(), m.publisher.commit.clone()),
            ("API range", self.api.as_str(), m.api.comment_form()),
        ];
        for (what, signed, manifest) in pairs {
            if signed != manifest {
                return Err(refuse(
                    Refusal::Mismatch,
                    format!("its manifest's {what} isn't the one that was signed"),
                ));
            }
        }
        Ok(())
    }
}

/// `checksums.json`, sorted and without repeats, every path a package path outside `pmmod/`.
fn parse_checksums(bytes: &[u8]) -> Result<BTreeMap<String, [u8; 32]>, PackageError> {
    let bad = |why: String| {
        refuse(
            Refusal::Checksums,
            format!("its checksums are malformed ({why})"),
        )
    };
    let raw: RawChecksums = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
    if raw.format != 1 {
        return Err(bad("unknown format".into()));
    }
    let mut out = BTreeMap::new();
    let mut previous: Option<&str> = None;
    for item in &raw.files {
        check_path(&item.path)?;
        if item.path.starts_with("pmmod/") {
            return Err(bad(format!("{} can't be listed", quote(&item.path))));
        }
        if previous.is_some_and(|p| p >= item.path.as_str()) {
            return Err(bad("not sorted, or a path is listed twice".into()));
        }
        previous = Some(&item.path);
        let hash = parse_sha256(&item.sha256)
            .ok_or_else(|| bad(format!("{}'s sha256", quote(&item.path))))?;
        out.insert(item.path.clone(), hash);
    }
    for required in [MANIFEST, LICENCE_FILE] {
        if !out.contains_key(required) {
            return Err(refuse(Refusal::Missing, format!("it has no {required}")));
        }
    }
    Ok(out)
}

/// 64 lowercase hex digits.
fn parse_sha256(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    hex::decode(s).ok()?.try_into().ok()
}

/// `LICENCE.txt` holds the AGPL's text (its title and version), so a mod can't ship under one
/// licence while its manifest names another.
fn check_licence_text(files: &BTreeMap<String, VerifiedFile>) -> Result<(), PackageError> {
    let text = files
        .get(LICENCE_FILE)
        .and_then(|f| std::str::from_utf8(&f.bytes).ok())
        .unwrap_or("");
    if text.contains("GNU AFFERO GENERAL PUBLIC LICENSE") && text.contains("Version 3") {
        Ok(())
    } else {
        Err(refuse(
            Refusal::Licence,
            format!("its {LICENCE_FILE} isn't the GNU AGPL, version 3"),
        ))
    }
}

// ---- The manifest ------------------------------------------------------------------------------

fn manifest_error(why: impl Into<String>) -> PackageError {
    refuse(
        Refusal::Manifest,
        format!("its manifest is invalid ({})", why.into()),
    )
}

/// Parse and check the manifest. `files` is the verified file set, for the paths it names.
fn parse_manifest(
    bytes: &[u8],
    files: &BTreeMap<String, VerifiedFile>,
) -> Result<Manifest, PackageError> {
    let raw: RawManifest =
        serde_json::from_slice(bytes).map_err(|e| manifest_error(e.to_string()))?;

    let id = ModId::parse(&raw.id).map_err(|e| refuse(Refusal::Id, e.to_string()))?;
    plain_text("name", &raw.name, 40, false, false)?;
    plain_text("description", &raw.description, 280, true, true)?;
    if let Some(icon) = &raw.icon {
        let raster_or_svg = ["png", "jpg", "jpeg", "webp", "gif", "svg"]
            .iter()
            .any(|ext| extension_is(icon, ext));
        if !raster_or_svg {
            return Err(manifest_error("icon must be a PNG, JPEG, WebP, GIF or SVG"));
        }
        listed_app_file("icon", icon, files)?;
    }
    let version = parse_version(&raw.version)?;
    let api = parse_api(&raw.api_version)?;
    let publisher = parse_publisher(&raw.publisher)?;
    if raw.licence != LICENCE {
        return Err(refuse(
            Refusal::Licence,
            format!("its licence must be {LICENCE}"),
        ));
    }

    // Permissions: known ids, each once, each with its reason.
    let mut granted = BTreeSet::new();
    for p in &raw.permissions {
        if !PERMISSIONS.contains(&p.id.as_str()) {
            return Err(manifest_error(format!(
                "unknown permission {}",
                quote(&p.id)
            )));
        }
        if !granted.insert(p.id.as_str()) {
            return Err(manifest_error(format!("{} is listed twice", quote(&p.id))));
        }
        plain_text("a permission's why", &p.why, 160, false, false)?;
    }

    let mut hosts = BTreeSet::new();
    if raw.internet_hosts.len() > 32 {
        return Err(manifest_error("too many internet hosts"));
    }
    for host in &raw.internet_hosts {
        if !is_internet_host(host) {
            return Err(manifest_error(format!(
                "{} isn't an internet host name",
                quote(host)
            )));
        }
        if !hosts.insert(host.as_str()) {
            return Err(manifest_error(format!("{} is listed twice", quote(host))));
        }
    }
    if hosts.is_empty() == granted.contains("internet") {
        return Err(manifest_error(
            "the internet permission and internetHosts go together",
        ));
    }

    let mut secret_names = BTreeSet::new();
    if raw.secrets.len() > 16 {
        return Err(manifest_error("too many secrets"));
    }
    for s in &raw.secrets {
        if !is_word(&s.name, 32, b"_-", true) || !s.name.as_bytes()[0].is_ascii_lowercase() {
            return Err(manifest_error(format!("secret name {}", quote(&s.name))));
        }
        if !secret_names.insert(s.name.as_str()) {
            return Err(manifest_error(format!(
                "secret {} is listed twice",
                quote(&s.name)
            )));
        }
        if !hosts.contains(s.host.as_str()) {
            return Err(manifest_error(format!(
                "secret {} goes to a host internetHosts doesn't list",
                quote(&s.name)
            )));
        }
        let header_ok = is_word(&s.header, 64, b"-", true)
            && s.header.as_bytes()[0].is_ascii_alphabetic()
            && !FORBIDDEN_SECRET_HEADERS.contains(&s.header.to_ascii_lowercase().as_str());
        if !header_ok {
            return Err(manifest_error(format!(
                "secret {} can't go in header {}",
                quote(&s.name),
                quote(&s.header)
            )));
        }
    }

    let contributions = parse_contributions(raw.contributions, &granted)?;
    check_data(&raw.data, &granted)?;

    listed_app_file("entry page", &raw.entry.page, files)?;
    if !extension_is(&raw.entry.page, "html") {
        return Err(manifest_error("the entry page must be an .html file"));
    }
    if raw.entry.workers.len() > 8 {
        return Err(manifest_error("too many workers"));
    }
    let mut workers = BTreeSet::new();
    for w in &raw.entry.workers {
        listed_app_file("worker", w, files)?;
        if !(extension_is(w, "js") || extension_is(w, "mjs")) {
            return Err(manifest_error(format!(
                "worker {} isn't a script",
                quote(w)
            )));
        }
        if !workers.insert(w.as_str()) {
            return Err(manifest_error(format!(
                "worker {} is listed twice",
                quote(w)
            )));
        }
    }

    let mut rooms = Vec::new();
    for r in &raw.requires {
        if rooms.contains(r) {
            return Err(manifest_error("a required room is listed twice"));
        }
        rooms.push(*r);
    }
    if let Some(engines) = &raw.engines {
        if !engines.frame {
            return Err(manifest_error("engines must be {\"frame\": true}"));
        }
    }

    Ok(Manifest {
        id,
        name: raw.name,
        description: raw.description,
        icon: raw.icon,
        version,
        api,
        publisher,
        permissions: raw.permissions,
        internet_hosts: raw.internet_hosts,
        secrets: raw.secrets,
        contributions,
        data: raw.data,
        entry: raw.entry,
        requires: rooms,
    })
}

/// Text PM draws as text: bounded, trimmed, and free of control characters and of the invisible
/// and direction-changing ones that would let a name pass for another (a mod's name is what PM's
/// band above its frame shows). Only a plain space is whitespace, plus a newline where allowed.
fn plain_text(
    what: &str,
    s: &str,
    max_chars: usize,
    newlines: bool,
    may_be_empty: bool,
) -> Result<(), PackageError> {
    if s.is_empty() {
        return if may_be_empty {
            Ok(())
        } else {
            Err(manifest_error(format!("{what} is empty")))
        };
    }
    if s.chars().count() > max_chars {
        return Err(manifest_error(format!(
            "{what} is longer than {max_chars} characters"
        )));
    }
    if s.trim() != s {
        return Err(manifest_error(format!(
            "{what} starts or ends with a space"
        )));
    }
    let bad = s.chars().any(|c| {
        let allowed_space = c == ' ' || (newlines && c == '\n');
        (c.is_whitespace() && !allowed_space)
            || (c.is_control() && !allowed_space)
            || is_invisible(c)
    });
    if bad {
        return Err(manifest_error(format!(
            "{what} has a hidden or control character"
        )));
    }
    Ok(())
}

/// Format and direction characters, zero-width ones, fillers, variation selectors and tags.
fn is_invisible(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x034F
            | 0x061C
            | 0x115F
            | 0x1160
            | 0x17B4
            | 0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x2028..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFFB
            | 0xE0000..=0xE0FFF
    )
}

/// ASCII letters and digits plus `extra`, 1..=`max` bytes; lowercase letters only unless `upper`.
fn is_word(s: &str, max: usize, extra: &[u8], upper: bool) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (upper && b.is_ascii_uppercase())
                || extra.contains(&b)
        })
}

fn extension_is(path: &str, ext: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(stem, e)| !stem.ends_with('/') && e.eq_ignore_ascii_case(ext))
}

/// A path the manifest names: a verified file under `app/`.
fn listed_app_file(
    what: &str,
    path: &str,
    files: &BTreeMap<String, VerifiedFile>,
) -> Result<(), PackageError> {
    if !path.starts_with(APP_DIR) || !files.contains_key(path) {
        return Err(manifest_error(format!(
            "the {what} {} isn't one of its app/ files",
            quote(path)
        )));
    }
    Ok(())
}

/// The mod's semver, in its one canonical spelling and without build metadata (which semver
/// ignores when comparing, so two different files could claim one version).
fn parse_version(s: &str) -> Result<semver::Version, PackageError> {
    let v = semver::Version::parse(s)
        .ok()
        .filter(|v| s.len() <= 64 && v.build.is_empty() && v.to_string() == s)
        .ok_or_else(|| manifest_error(format!("version {} isn't a plain semver", quote(s))))?;
    Ok(v)
}

/// `major.minor`, no leading zeros.
fn parse_major_minor(s: &str) -> Option<(u16, u16)> {
    let (a, b) = s.split_once('.')?;
    let part = |p: &str| -> Option<u16> {
        let ok = !p.is_empty()
            && p.len() <= 4
            && p.bytes().all(|b| b.is_ascii_digit())
            && (p == "0" || !p.starts_with('0'));
        ok.then(|| p.parse().ok()).flatten()
    };
    Some((part(a)?, part(b)?))
}

fn parse_api(raw: &RawApi) -> Result<ApiRange, PackageError> {
    let min = parse_major_minor(&raw.min);
    let target = parse_major_minor(&raw.target);
    match (min, target) {
        (Some(min), Some(target)) if min <= target => Ok(ApiRange { min, target }),
        _ => Err(manifest_error(
            "apiVersion must be {\"min\": \"M.m\", \"target\": \"M.m\"} with min <= target",
        )),
    }
}

fn parse_publisher(raw: &RawPublisher) -> Result<Publisher, PackageError> {
    let tier = Tier::parse(&raw.tier).ok_or_else(|| manifest_error("publisher.tier"))?;
    plain_text("publisher.author", &raw.author, 64, false, false)?;
    if !is_source_repo(&raw.source_repo) {
        return Err(manifest_error("publisher.sourceRepo must be owner/repo"));
    }
    if !is_commit(&raw.commit) {
        return Err(manifest_error(
            "publisher.commit must be a 40-digit lowercase hex commit",
        ));
    }
    Ok(Publisher {
        tier,
        author: raw.author.clone(),
        source_repo: raw.source_repo.clone(),
        commit: raw.commit.clone(),
    })
}

/// A git commit as PM writes one: 40 lowercase hex digits (a SHA-1 object id).
fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// GitHub's `owner/repo` shape.
fn is_source_repo(s: &str) -> bool {
    let Some((owner, repo)) = s.split_once('/') else {
        return false;
    };
    is_word(owner, 39, b"-", true)
        && !owner.starts_with('-')
        && is_word(repo, 100, b"._-", true)
        && repo != "."
        && repo != ".."
}

/// An exact DNS host name: lowercase labels, no IP literal, no port, no wildcard, nothing local.
fn is_internet_host(h: &str) -> bool {
    if h.len() > 253 || h == "localhost" || h.ends_with(".localhost") {
        return false;
    }
    let labels: Vec<&str> = h.split('.').collect();
    let labels_ok = labels.len() >= 2
        && labels
            .iter()
            .all(|l| is_word(l, 63, b"-", false) && !l.starts_with('-') && !l.ends_with('-'));
    // An all-digit last label is an IPv4 literal, never a public suffix.
    labels_ok && !labels[labels.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

fn parse_contributions(
    raw: RawContributions,
    granted: &BTreeSet<&str>,
) -> Result<Contributions, PackageError> {
    if let Some(tab) = &raw.nav_tab {
        plain_text("nav.tab's label", &tab.label, 24, false, false)?;
        if !granted.contains("ui.tab") {
            return Err(manifest_error("a nav.tab needs the ui.tab permission"));
        }
    }
    if let Some(overlay) = &raw.calendar_overlay {
        plain_text("calendar.overlay's label", &overlay.label, 40, false, false)?;
    }
    if raw.help.len() > 64 {
        return Err(manifest_error("too many help entries"));
    }
    let mut ids = BTreeSet::new();
    for h in &raw.help {
        let id_ok = is_word(&h.id, 48, b"-", false) && h.id.as_bytes()[0].is_ascii_lowercase();
        if !id_ok || !ids.insert(h.id.as_str()) {
            return Err(manifest_error(format!("help id {}", quote(&h.id))));
        }
        plain_text("a help title", &h.title, 80, false, false)?;
        plain_text("a help body", &h.body, 2000, true, false)?;
    }
    Ok(Contributions {
        nav_tab: raw.nav_tab.map(|t| t.label),
        calendar_overlay: raw.calendar_overlay.map(|o| o.label),
        help: raw.help,
    })
}

fn check_data(data: &DataDecl, granted: &BTreeSet<&str>) -> Result<(), PackageError> {
    if !data.keys.is_empty() && !granted.contains("storage") {
        return Err(manifest_error("data keys need the storage permission"));
    }
    if data.keys.len() > 64 || data.project_refs.len() > 64 {
        return Err(manifest_error("too many data keys or project refs"));
    }
    let mut keys = BTreeSet::new();
    for k in &data.keys {
        let ok = is_word(&k.key, 64, b"_.-", false) && k.key.as_bytes()[0].is_ascii_lowercase();
        if !ok || !keys.insert(k.key.as_str()) {
            return Err(manifest_error(format!("data key {}", quote(&k.key))));
        }
    }
    for r in &data.project_refs {
        if !keys.contains(r.key.as_str()) {
            return Err(manifest_error(format!(
                "a projectRef names {}, which data.keys doesn't declare",
                quote(&r.key)
            )));
        }
        let path_ok = !r.path.is_empty()
            && r.path.len() <= 256
            && r.path.bytes().all(|b| b.is_ascii_graphic());
        if !path_ok {
            return Err(manifest_error(format!(
                "projectRef path {}",
                quote(&r.path)
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use blake2::Blake2b512;
    use ring::rand::{SecureRandom, SystemRandom};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::{json, Value};
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    type Zw = ZipWriter<Cursor<Vec<u8>>>;

    // ---- Signing, in the test: minisign's format with a key made for this run only (no key is
    // ever committed) -------------------------------------------------------------------------

    struct TestKey {
        pair: Ed25519KeyPair,
        id: [u8; 8],
    }

    impl TestKey {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
            let mut id = [0u8; 8];
            rng.fill(&mut id).unwrap();
            Self { pair, id }
        }

        /// The minisign public key ("Ed", key id, key).
        fn public(&self) -> PublicKey {
            let mut bin = vec![0x45, 0x64];
            bin.extend_from_slice(&self.id);
            bin.extend_from_slice(self.pair.public_key().as_ref());
            PublicKey::from_base64(&B64.encode(bin)).unwrap()
        }

        /// A prehashed ("ED") minisign signature: Ed25519 over BLAKE2b-512 of the message, then a
        /// global signature over that signature and the trusted comment.
        fn sign(&self, message: &[u8], comment: &str) -> String {
            let hash = Blake2b512::digest(message);
            self.sign_raw(&hash, [0x45, 0x44], comment)
        }

        /// minisign's legacy mode: Ed25519 over the message itself.
        fn sign_legacy(&self, message: &[u8], comment: &str) -> String {
            self.sign_raw(message, [0x45, 0x64], comment)
        }

        fn sign_raw(&self, signed: &[u8], alg: [u8; 2], comment: &str) -> String {
            let sig = self.pair.sign(signed);
            let mut bin = alg.to_vec();
            bin.extend_from_slice(&self.id);
            bin.extend_from_slice(sig.as_ref());
            let mut global = sig.as_ref().to_vec();
            global.extend_from_slice(comment.as_bytes());
            let global = self.pair.sign(&global);
            format!(
                "untrusted comment: test signature\n{}\ntrusted comment: {comment}\n{}\n",
                B64.encode(bin),
                B64.encode(global.as_ref())
            )
        }
    }

    const AGPL: &str =
        "                    GNU AFFERO GENERAL PUBLIC LICENSE\n                       \
                        Version 3, 19 November 2007\n";
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn good_manifest() -> Value {
        json!({
            "id": "atlas.map",
            "name": "Map",
            "description": "Your documents, laid out by meaning.",
            "icon": "app/icon.png",
            "version": "1.0.0",
            "apiVersion": {"min": "1.0", "target": "1.0"},
            "publisher": {
                "tier": "first",
                "author": "Atlas",
                "sourceRepo": "itsAtlas-org/Personal-Manager",
                "commit": COMMIT
            },
            "licence": "AGPL-3.0-or-later",
            "permissions": [
                {"id": "storage", "why": "Remembers where you left the map."},
                {"id": "ui.tab", "why": "Its own tab."},
                {"id": "documents.meta", "why": "Shows your documents' titles."}
            ],
            "contributions": {
                "nav.tab": {"label": "Map"},
                "help": [{"id": "map-canvas", "title": "The map", "body": "Drag to pan.\nScroll to zoom."}]
            },
            "data": {"keys": [{"key": "settings", "class": "content"}]},
            "entry": {"page": "app/index.html", "workers": ["app/layout.js"]},
            "requires": [],
            "engines": {"frame": true}
        })
    }

    fn comment_for(m: &Value) -> String {
        format!(
            "pmmod:1\tid:{}\tversion:{}\ttier:{}\tauthor:{}\tsourceRepo:{}\tcommit:{}\tapi:{}-{}",
            m["id"].as_str().unwrap(),
            m["version"].as_str().unwrap(),
            m["publisher"]["tier"].as_str().unwrap(),
            m["publisher"]["author"].as_str().unwrap(),
            m["publisher"]["sourceRepo"].as_str().unwrap(),
            m["publisher"]["commit"].as_str().unwrap(),
            m["apiVersion"]["min"].as_str().unwrap(),
            m["apiVersion"]["target"].as_str().unwrap(),
        )
    }

    fn checksums_json(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let list: Vec<Value> = files
            .iter()
            .map(|(p, b)| json!({"path": p, "sha256": hex::encode(Sha256::digest(b))}))
            .collect();
        serde_json::to_vec(&json!({"format": 1, "files": list})).unwrap()
    }

    /// A package, built from parts each test can change before calling [`Fixture::bytes`].
    struct Fixture {
        key: TestKey,
        manifest: Value,
        /// Every checksummed file but the manifest, which comes from `manifest`.
        files: BTreeMap<String, Vec<u8>>,
        /// `None`: derived from the manifest.
        comment: Option<String>,
        /// The checksums file's bytes, when a test writes its own.
        checksums: Option<Vec<u8>>,
        /// The signature file's bytes, when a test writes its own (`Some(None)`: no signature).
        signature: Option<Option<Vec<u8>>>,
        /// Written into the zip in place of a file's checksummed bytes.
        tampered: BTreeMap<String, Vec<u8>>,
        /// In the zip, not in the checksums.
        extra: Vec<(String, Vec<u8>)>,
        /// In the checksums, not in the zip.
        omitted: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut files = BTreeMap::new();
            let starting: [(&str, &[u8]); 6] = [
                ("LICENCE.txt", AGPL.as_bytes()),
                ("NOTICE.txt", b"No bundled packages.\n"),
                (
                    "app/index.html",
                    b"<!doctype html><script src=\"main.js\"></script>",
                ),
                ("app/main.js", b"console.log('map');"),
                ("app/layout.js", b"onmessage = () => {};"),
                ("app/icon.png", b"\x89PNG fake"),
            ];
            for (p, b) in starting {
                files.insert(p.to_owned(), b.to_vec());
            }
            Self {
                key: TestKey::new(),
                manifest: good_manifest(),
                files,
                comment: None,
                checksums: None,
                signature: None,
                tampered: BTreeMap::new(),
                extra: Vec::new(),
                omitted: Vec::new(),
            }
        }

        fn listed(&self) -> BTreeMap<String, Vec<u8>> {
            let mut all = self.files.clone();
            all.insert(
                MANIFEST.to_owned(),
                serde_json::to_vec(&self.manifest).unwrap(),
            );
            all
        }

        fn checksums_bytes(&self) -> Vec<u8> {
            self.checksums
                .clone()
                .unwrap_or_else(|| checksums_json(&self.listed()))
        }

        fn comment(&self) -> String {
            self.comment
                .clone()
                .unwrap_or_else(|| comment_for(&self.manifest))
        }

        fn signature_bytes(&self) -> Option<Vec<u8>> {
            match &self.signature {
                Some(s) => s.clone(),
                None => Some(
                    self.key
                        .sign(&self.checksums_bytes(), &self.comment())
                        .into_bytes(),
                ),
            }
        }

        /// The zip entries, in order.
        fn entries(&self) -> Vec<(String, Vec<u8>)> {
            let mut out = Vec::new();
            for (path, bytes) in self.listed() {
                if self.omitted.contains(&path) {
                    continue;
                }
                let bytes = self.tampered.get(&path).cloned().unwrap_or(bytes);
                out.push((path, bytes));
            }
            out.extend(self.extra.iter().cloned());
            out.push((CHECKSUMS.to_owned(), self.checksums_bytes()));
            if let Some(sig) = self.signature_bytes() {
                out.push((SIGNATURE.to_owned(), sig));
            }
            out
        }

        fn bytes(&self) -> Vec<u8> {
            self.bytes_with(|_| {})
        }

        /// The zip, with `more` adding entries after the regular ones.
        fn bytes_with(&self, more: impl FnOnce(&mut Zw)) -> Vec<u8> {
            let mut zw = ZipWriter::new(Cursor::new(Vec::new()));
            for (path, bytes) in self.entries() {
                zw.start_file(path, SimpleFileOptions::default()).unwrap();
                zw.write_all(&bytes).unwrap();
            }
            more(&mut zw);
            zw.finish().unwrap().into_inner()
        }

        fn verify(&self) -> Result<VerifiedPackage, PackageError> {
            verify(&self.bytes(), &[self.key.public()])
        }
    }

    fn refusal(r: Result<VerifiedPackage, PackageError>) -> Refusal {
        match r {
            Ok(_) => panic!("the package was accepted"),
            Err(e) => e.refusal,
        }
    }

    fn with_manifest(change: impl FnOnce(&mut Value)) -> Fixture {
        let mut f = Fixture::new();
        change(&mut f.manifest);
        f
    }

    /// Rewrite a field in an entry's local and central headers alike (so they still agree).
    fn patch_headers(zip: &mut [u8], name: &str, local_at: usize, central_at: usize, value: &[u8]) {
        let mut patched = 0;
        let mut i = 0;
        while i + 4 <= zip.len() {
            let (name_at, kind) = match &zip[i..i + 4] {
                b"PK\x03\x04" => (30, local_at),
                b"PK\x01\x02" => (46, central_at),
                _ => {
                    i += 1;
                    continue;
                }
            };
            let len_at = if name_at == 30 { 26 } else { 28 };
            let len = u16::from_le_bytes([zip[i + len_at], zip[i + len_at + 1]]) as usize;
            if zip.get(i + name_at..i + name_at + len) == Some(name.as_bytes()) {
                zip[i + kind..i + kind + value.len()].copy_from_slice(value);
                patched += 1;
            }
            i += 4;
        }
        assert_eq!(patched, 2, "{name} has a local and a central header");
    }

    // ---- What passes -------------------------------------------------------------------------

    #[test]
    fn a_well_formed_signed_package_verifies() {
        let f = Fixture::new();
        let other = TestKey::new();
        // Any trusted key may have signed it; the result says which.
        let pkg = verify(&f.bytes(), &[other.public(), f.key.public()]).unwrap();
        assert_eq!(pkg.signer, 1);
        let m = &pkg.manifest;
        assert_eq!(m.id.as_str(), "atlas.map");
        assert_eq!(m.name, "Map");
        assert_eq!(m.description, "Your documents, laid out by meaning.");
        assert_eq!(m.icon.as_deref(), Some("app/icon.png"));
        assert!(m.internet_hosts.is_empty() && m.secrets.is_empty() && m.requires.is_empty());
        assert_eq!(m.version, semver::Version::new(1, 0, 0));
        assert_eq!(m.publisher.tier, Tier::First);
        assert_eq!(
            m.api,
            ApiRange {
                min: (1, 0),
                target: (1, 0)
            }
        );
        assert_eq!(m.entry.page, "app/index.html");
        assert_eq!(m.contributions.nav_tab.as_deref(), Some("Map"));
        assert_eq!(m.data.keys[0].class, DataClass::Content);
        // Exactly the checksummed files, as their verified bytes.
        let paths: Vec<&str> = pkg.files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            [
                "LICENCE.txt",
                "NOTICE.txt",
                "app/icon.png",
                "app/index.html",
                "app/layout.js",
                "app/main.js",
                "manifest.json"
            ]
        );
        let main = &pkg.files["app/main.js"];
        assert_eq!(main.bytes, b"console.log('map');");
        assert_eq!(main.sha256, <[u8; 32]>::from(Sha256::digest(&main.bytes)));
        assert_eq!(pkg.trusted_comment, comment_for(&f.manifest));
    }

    /// The whole path a slice-3 install will take: verify, write the verified bytes, register, open
    /// a frame, serve its entry and a script.
    #[test]
    fn a_verified_package_installs_and_serves() {
        use super::super::engine::Platform;
        use super::super::runtime::{InstalledMod, ModRuntime};
        let f = Fixture::new();
        let pkg = f.verify().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (path, file) in &pkg.files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, &file.bytes).unwrap();
        }
        let runtime = ModRuntime::new(Platform::Linux, vec!["tauri://localhost".into()]);
        runtime.install(InstalledMod::from_package(&pkg, dir.path()).unwrap());
        let opened = runtime.open_frame(&pkg.manifest.id).unwrap();
        let get = |uri: &str| {
            let req = tauri::http::Request::builder()
                .uri(uri)
                .body(Vec::new())
                .unwrap();
            runtime.serve("main", &req)
        };
        let page = get(&opened.entry_url);
        assert_eq!(page.status(), 200);
        assert_eq!(page.body(), &f.files["app/index.html"]);
        assert!(page.headers().get("Content-Disposition").is_none());
        assert_eq!(
            runtime.session_mod(&opened.session),
            Ok(pkg.manifest.id.clone())
        );
        let script = get("pmmod://localhost/atlas.map/app/main.js");
        assert_eq!(script.body(), &f.files["app/main.js"]);
        // Only app/ is served: the manifest and licence are the package's, not the frame's.
        assert_eq!(
            get("pmmod://localhost/atlas.map/manifest.json").status(),
            404
        );
    }

    #[test]
    fn a_second_party_package_and_a_minimal_manifest_verify() {
        let f = with_manifest(|m| {
            *m = json!({
                "id": "acme.timer",
                "name": "Timer",
                "description": "",
                "version": "0.1.0-beta.2",
                "apiVersion": {"min": "1.0", "target": "1.3"},
                "publisher": {"tier": "second", "author": "Acme Ltd", "sourceRepo": "acme/timer", "commit": COMMIT},
                "licence": "AGPL-3.0-or-later",
                "entry": {"page": "app/index.html"}
            });
        });
        let pkg = f.verify().unwrap();
        assert_eq!(pkg.manifest.publisher.tier, Tier::Second);
        assert!(pkg.manifest.permissions.is_empty());
        assert_eq!(pkg.manifest.api.comment_form(), "1.0-1.3");
    }

    // ---- The signature -----------------------------------------------------------------------

    #[test]
    fn a_valid_signature_by_an_untrusted_key_is_refused() {
        let f = Fixture::new();
        let stranger = TestKey::new();
        assert_eq!(
            refusal(verify(&f.bytes(), &[stranger.public()])),
            Refusal::UntrustedKey
        );
        // No keys at all (before slice 5 embeds them): nothing verifies.
        assert_eq!(refusal(verify(&f.bytes(), &[])), Refusal::UntrustedKey);
    }

    #[test]
    fn a_forged_signature_under_a_trusted_key_id_is_refused() {
        let mut f = Fixture::new();
        let forger = TestKey {
            id: f.key.id,
            ..TestKey::new()
        };
        f.signature = Some(Some(
            forger.sign(&f.checksums_bytes(), &f.comment()).into_bytes(),
        ));
        assert_eq!(refusal(f.verify()), Refusal::BadSignature);
    }

    #[test]
    fn a_signature_over_other_bytes_is_refused() {
        let mut f = Fixture::new();
        f.signature = Some(Some(
            f.key.sign(b"something else", &f.comment()).into_bytes(),
        ));
        assert_eq!(refusal(f.verify()), Refusal::BadSignature);
    }

    #[test]
    fn an_edited_trusted_comment_breaks_the_signature() {
        let mut f = Fixture::new();
        let sig = f.key.sign(&f.checksums_bytes(), &f.comment());
        let edited = sig.replace("version:1.0.0", "version:9.0.0");
        assert_ne!(sig, edited);
        f.signature = Some(Some(edited.into_bytes()));
        assert_eq!(refusal(f.verify()), Refusal::BadSignature);
    }

    #[test]
    fn a_legacy_signature_is_refused() {
        let mut f = Fixture::new();
        f.signature = Some(Some(
            f.key
                .sign_legacy(&f.checksums_bytes(), &f.comment())
                .into_bytes(),
        ));
        assert_eq!(refusal(f.verify()), Refusal::Signature);
    }

    #[test]
    fn an_unsigned_or_malformed_signature_is_refused() {
        let mut f = Fixture::new();
        f.signature = Some(None);
        assert_eq!(refusal(f.verify()), Refusal::Unsigned);
        for garbage in [b"not a signature".to_vec(), vec![0xff, 0xfe], Vec::new()] {
            f.signature = Some(Some(garbage));
            assert_eq!(refusal(f.verify()), Refusal::Signature);
        }
        // A good signature with a fifth line after it.
        let mut sig = f.key.sign(&f.checksums_bytes(), &f.comment());
        sig.push_str("trailing\n");
        f.signature = Some(Some(sig.into_bytes()));
        assert_eq!(refusal(f.verify()), Refusal::Signature);
    }

    /// The exact text the pm-mods sign job has to write, pinned here rather than only through the
    /// fixture that builds it.
    #[test]
    fn the_trusted_comment_is_eight_fields_in_a_fixed_order() {
        let comment = comment_for(&good_manifest());
        assert_eq!(
            comment,
            "pmmod:1\tid:atlas.map\tversion:1.0.0\ttier:first\tauthor:Atlas\t\
             sourceRepo:itsAtlas-org/Personal-Manager\t\
             commit:0123456789abcdef0123456789abcdef01234567\tapi:1.0-1.0"
        );
        let f = Fixture::new();
        assert_eq!(f.verify().unwrap().trusted_comment, comment);
    }

    #[test]
    fn a_malformed_trusted_comment_is_refused() {
        let good = comment_for(&good_manifest());
        let commit = format!("commit:{COMMIT}");
        for comment in [
            String::new(),
            good.replace("pmmod:1\t", ""),
            good.replace("pmmod:1", "pmmod:2"),
            good.replace("pmmod:1", "seq:1"),
            good.replace('\t', " "),
            good.replace("\tid:", "\tname:"),
            format!("{good}\textra:1"),
            good.replace("tier:first", "tier:third"),
            good.replace("tier:first", "tier:First"),
            good.replace("author:Atlas", "author:"),
            // Fields out of order.
            good.replace(
                "\tid:atlas.map\tversion:1.0.0",
                "\tversion:1.0.0\tid:atlas.map",
            ),
            // The commit: missing, after the API range, twice, under another key, or not 40
            // lowercase hex (empty, short, long, upper case, not hex, a SHA-256 object id).
            good.replace(&format!("\t{commit}"), ""),
            good.replace(
                &format!("\t{commit}\tapi:1.0-1.0"),
                &format!("\tapi:1.0-1.0\t{commit}"),
            ),
            good.replace("\tapi:1.0-1.0", &format!("\t{commit}")),
            good.replace("\tcommit:", "\tcommitSha:"),
            good.replace(&commit, "commit:"),
            good.replace(&commit, &commit[..commit.len() - 1]),
            good.replace(&commit, &format!("{commit}0")),
            good.replace(&commit, &commit.replace('a', "A")),
            good.replace(&commit, &commit.replace('a', "g")),
            good.replace(&commit, &format!("commit:{}", "a".repeat(64))),
        ] {
            assert_ne!(comment, good);
            let mut f = Fixture::new();
            f.comment = Some(comment.clone());
            assert_eq!(refusal(f.verify()), Refusal::TrustedComment, "{comment:?}");
        }
    }

    #[test]
    fn a_trusted_comment_that_disagrees_with_the_manifest_is_refused() {
        let good = comment_for(&good_manifest());
        for (from, to) in [
            ("id:atlas.map", "id:atlas.other"),
            ("version:1.0.0", "version:1.0.1"),
            ("tier:first", "tier:second"),
            ("author:Atlas", "author:Atlas Ltd"),
            (
                "sourceRepo:itsAtlas-org/Personal-Manager",
                "sourceRepo:itsAtlas-org/pm-mods",
            ),
            ("api:1.0-1.0", "api:1.0-1.1"),
            // A well-formed commit, but not the manifest's.
            (
                "commit:0123456789abcdef0123456789abcdef01234567",
                "commit:0123456789abcdef0123456789abcdef01234568",
            ),
        ] {
            let mut f = Fixture::new();
            let comment = good.replace(from, to);
            assert_ne!(comment, good);
            f.comment = Some(comment);
            assert_eq!(refusal(f.verify()), Refusal::Mismatch, "{to}");
        }
        // And the other way round: the manifest names another commit than the one signed.
        let mut f = with_manifest(|m| m["publisher"]["commit"] = json!("f".repeat(40)));
        f.comment = Some(good);
        assert_eq!(refusal(f.verify()), Refusal::Mismatch);
    }

    // ---- The file set ------------------------------------------------------------------------

    #[test]
    fn a_tampered_file_is_refused() {
        let mut f = Fixture::new();
        f.tampered.insert(
            "app/main.js".into(),
            b"fetch('https://evil.example')".to_vec(),
        );
        assert_eq!(refusal(f.verify()), Refusal::ChecksumMismatch);
        // The manifest too: it is checksummed like any file.
        let mut f = Fixture::new();
        let mut m = f.manifest.clone();
        m["permissions"][2]["id"] = json!("documents.text");
        f.tampered
            .insert(MANIFEST.into(), serde_json::to_vec(&m).unwrap());
        assert_eq!(refusal(f.verify()), Refusal::ChecksumMismatch);
    }

    #[test]
    fn an_extra_file_is_refused() {
        let mut f = Fixture::new();
        f.extra.push(("app/extra.js".into(), b"1".to_vec()));
        assert_eq!(refusal(f.verify()), Refusal::Unlisted);
    }

    #[test]
    fn a_missing_file_is_refused() {
        let mut f = Fixture::new();
        f.omitted.push("app/main.js".into());
        assert_eq!(refusal(f.verify()), Refusal::Missing);
        let mut f = Fixture::new();
        f.omitted.push(MANIFEST.into());
        assert_eq!(refusal(f.verify()), Refusal::Missing);
    }

    #[test]
    fn checksums_must_be_well_formed_sorted_and_complete() {
        let f = Fixture::new();
        let listed = f.listed();
        let entry = |p: &str| json!({"path": p, "sha256": hex::encode(Sha256::digest(&listed[p]))});
        let all: Vec<Value> = listed.keys().map(|p| entry(p.as_str())).collect();
        let mut reversed = all.clone();
        reversed.reverse();
        let mut twice = all.clone();
        twice.insert(1, all[0].clone());
        let mut upper = all.clone();
        upper[0]["sha256"] = json!(upper[0]["sha256"].as_str().unwrap().to_uppercase());
        let mut short = all.clone();
        short[0]["sha256"] = json!("abc");
        let mut listing_pmmod = all.clone();
        listing_pmmod.push(json!({"path": CHECKSUMS, "sha256": "0".repeat(64)}));
        for (what, checksums) in [
            ("format", json!({"format": 2, "files": all})),
            ("unknown key", json!({"format": 1, "files": all, "x": 1})),
            ("unsorted", json!({"format": 1, "files": reversed})),
            ("twice", json!({"format": 1, "files": twice})),
            ("uppercase hex", json!({"format": 1, "files": upper})),
            ("short hash", json!({"format": 1, "files": short})),
            ("lists pmmod/", json!({"format": 1, "files": listing_pmmod})),
            ("not an object", json!([])),
        ] {
            let mut f = Fixture::new();
            f.checksums = Some(serde_json::to_vec(&checksums).unwrap());
            assert_eq!(refusal(f.verify()), Refusal::Checksums, "{what}");
        }
        // A listed path that breaks the path rules is refused by them.
        let mut bad_path = all.clone();
        bad_path.push(json!({"path": "app/../x", "sha256": "0".repeat(64)}));
        let mut f = Fixture::new();
        f.checksums = Some(serde_json::to_vec(&json!({"format": 1, "files": bad_path})).unwrap());
        assert_eq!(refusal(f.verify()), Refusal::EntryName);
        // Without the licence file.
        let mut f = Fixture::new();
        f.files.remove(LICENCE_FILE);
        assert_eq!(refusal(f.verify()), Refusal::Missing);
    }

    // ---- The zip -----------------------------------------------------------------------------

    #[test]
    fn bad_entry_names_are_refused() {
        for name in [
            "/app/x.js",
            "app/../manifest.json",
            "app/./x.js",
            "../x.js",
            "app//x.js",
            "app\\x.js",
            "C:/app/x.js",
            "app/x.js:stream",
            "app/x.js.",
            "app/x.js ",
            "app/x y.js",
            "app/con.js",
            "app/NUL",
            "app/com1.txt",
            "app/lpt9.x",
            "app/aux/x.js",
            "app/PROGRA~1.js",
            "app/é.js",
            "app/x%2e.js",
            "app/x?.js",
            "app/x\0.js",
            "other/x.js",
            "pmmod/extra.json",
            "README.md",
            &format!("app/{}", "a".repeat(101)),
            &format!("app/{}x.js", "a/".repeat(16)),
        ] {
            let f = Fixture::new();
            let bytes = f.bytes_with(|zw| {
                zw.start_file(name, SimpleFileOptions::default()).unwrap();
                zw.write_all(b"x").unwrap();
            });
            assert_eq!(
                refusal(verify(&bytes, &[f.key.public()])),
                Refusal::EntryName,
                "{name:?}"
            );
        }
    }

    #[test]
    fn folders_symlinks_and_special_files_are_refused() {
        let f = Fixture::new();
        let opts = SimpleFileOptions::default();
        type AddEntry = Box<dyn Fn(&mut Zw)>;
        let cases: Vec<AddEntry> = vec![
            Box::new(move |zw| zw.add_directory("app/sub/", opts).unwrap()),
            Box::new(move |zw| {
                zw.add_symlink("app/link.js", "../../etc/passwd", opts)
                    .unwrap()
            }),
            Box::new(move |zw| {
                // A FIFO's mode bits.
                let fifo = opts
                    .system(zip::System::Unix)
                    .external_attributes(0o010644 << 16);
                zw.start_file("app/fifo", fifo).unwrap();
            }),
        ];
        for (i, add) in cases.into_iter().enumerate() {
            let bytes = f.bytes_with(|zw| add(zw));
            assert_eq!(
                refusal(verify(&bytes, &[f.key.public()])),
                Refusal::NotAFile,
                "case {i}"
            );
        }
    }

    #[test]
    fn names_equal_after_case_folding_are_refused() {
        let f = Fixture::new();
        for (a, b) in [("app/Main.js", "app/main.JS"), ("app/x", "app/X/y.js")] {
            let bytes = f.bytes_with(|zw| {
                for name in [a, b] {
                    zw.start_file(name, SimpleFileOptions::default()).unwrap();
                }
            });
            assert_eq!(
                refusal(verify(&bytes, &[f.key.public()])),
                Refusal::CaseCollision,
                "{a} {b}"
            );
        }
        // Against a listed file too.
        let bytes = f.bytes_with(|zw| {
            zw.start_file("APP/main.js", SimpleFileOptions::default())
                .unwrap();
        });
        assert_eq!(
            refusal(verify(&bytes, &[f.key.public()])),
            Refusal::EntryName,
            "APP/ isn't app/"
        );
        let bytes = f.bytes_with(|zw| {
            zw.start_file("app/MAIN.js", SimpleFileOptions::default())
                .unwrap();
        });
        assert_eq!(
            refusal(verify(&bytes, &[f.key.public()])),
            Refusal::CaseCollision
        );
    }

    #[test]
    fn a_duplicate_entry_is_refused() {
        let f = Fixture::new();
        let mut bytes = f.bytes_with(|zw| {
            for name in ["app/dup-a.js", "app/dup-b.js"] {
                zw.start_file(name, SimpleFileOptions::default()).unwrap();
                zw.write_all(b"x").unwrap();
            }
        });
        // Rename the second to the first in both its headers.
        let from = b"app/dup-b.js";
        let mut i = 0;
        while let Some(at) = bytes[i..].windows(from.len()).position(|w| w == from) {
            bytes[i + at..i + at + from.len()].copy_from_slice(b"app/dup-a.js");
            i += at + from.len();
        }
        assert_eq!(
            refusal(verify(&bytes, &[f.key.public()])),
            Refusal::ZipLayout
        );
    }

    #[test]
    fn an_encrypted_entry_is_refused() {
        let f = Fixture::new();
        let mut bytes = f.bytes();
        // General-purpose flag bit 0, in both headers.
        patch_headers(&mut bytes, "app/main.js", 6, 8, &[0x01, 0x00]);
        assert_eq!(
            refusal(verify(&bytes, &[f.key.public()])),
            Refusal::Encrypted
        );
    }

    #[test]
    fn only_stored_and_deflated_entries_are_read() {
        let f = Fixture::new();
        let mut bytes = f.bytes();
        // Method 12 (bzip2), in both headers.
        patch_headers(&mut bytes, "app/main.js", 8, 10, &[12, 0]);
        assert_eq!(
            refusal(verify(&bytes, &[f.key.public()])),
            Refusal::Compression
        );
        // Stored passes like deflated.
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut zw = ZipWriter::new(Cursor::new(Vec::new()));
        for (path, data) in f.entries() {
            zw.start_file(path, stored).unwrap();
            zw.write_all(&data).unwrap();
        }
        let stored_zip = zw.finish().unwrap().into_inner();
        assert!(verify(&stored_zip, &[f.key.public()]).is_ok());
    }

    #[test]
    fn junk_before_the_zip_or_no_zip_at_all_is_refused() {
        let f = Fixture::new();
        let mut prefixed = b"MZ junk".to_vec();
        prefixed.extend(f.bytes());
        assert_eq!(
            refusal(verify(&prefixed, &[f.key.public()])),
            Refusal::NotAZip
        );
        for bytes in [Vec::new(), b"PK".to_vec(), vec![0u8; 1024]] {
            assert_eq!(refusal(verify(&bytes, &[])), Refusal::NotAZip);
        }
    }

    // ---- Caps, on the bytes actually decompressed ---------------------------------------------

    const SMALL: Limits = Limits {
        file: 1 << 20,
        entries: 32,
        entry: 4 << 10,
        total: 24 << 10,
    };

    #[test]
    fn the_file_and_entry_count_are_capped() {
        let f = Fixture::new();
        let bytes = f.bytes();
        let tight = Limits {
            file: bytes.len() - 1,
            ..SMALL
        };
        assert_eq!(
            refusal(verify_with(&bytes, &[f.key.public()], &tight)),
            Refusal::TooLarge
        );
        let few = Limits {
            entries: 3,
            ..SMALL
        };
        assert_eq!(
            refusal(verify_with(&bytes, &[f.key.public()], &few)),
            Refusal::TooManyEntries
        );
        assert!(verify_with(&bytes, &[f.key.public()], &SMALL).is_ok());
    }

    #[test]
    fn a_zip_bomb_entry_stops_at_the_cap() {
        // A megabyte of zeros deflates to about a kilobyte.
        let mut f = Fixture::new();
        f.files.insert("app/bomb.js".into(), vec![0u8; 1 << 20]);
        let bytes = f.bytes();
        assert!(bytes.len() < 16 << 10);
        assert_eq!(
            refusal(verify_with(&bytes, &[f.key.public()], &SMALL)),
            Refusal::EntryTooLarge
        );
        // Exactly at the cap passes.
        let mut f = Fixture::new();
        f.files
            .insert("app/full.js".into(), vec![b'x'; SMALL.entry]);
        assert!(verify_with(&f.bytes(), &[f.key.public()], &SMALL).is_ok());
    }

    #[test]
    fn many_entries_under_the_cap_still_hit_the_total() {
        let mut f = Fixture::new();
        for i in 0..8 {
            f.files
                .insert(format!("app/part{i}.js"), vec![b'x'; SMALL.entry - 1]);
        }
        assert_eq!(
            refusal(verify_with(&f.bytes(), &[f.key.public()], &SMALL)),
            Refusal::TotalTooLarge
        );
    }

    #[test]
    fn a_declared_size_is_never_trusted() {
        // The headers say ten bytes; the data inflates past the cap.
        let mut f = Fixture::new();
        f.files.insert("app/liar.js".into(), vec![b'y'; 64 << 10]);
        let mut bytes = f.bytes();
        patch_headers(&mut bytes, "app/liar.js", 22, 24, &10u32.to_le_bytes());
        // Either the reader stops at the declared size, and the CRC then fails, or it reads on and
        // the cap stops it. Never a ten-byte file accepted.
        let r = refusal(verify_with(&bytes, &[f.key.public()], &SMALL));
        assert!(
            matches!(r, Refusal::EntryTooLarge | Refusal::Corrupt),
            "{r:?}"
        );
    }

    #[test]
    fn the_manifest_and_signature_have_their_own_small_caps() {
        // Read before it is parsed, so the cap holds whatever the manifest says.
        let mut f = Fixture::new();
        f.manifest["padding"] = json!("x".repeat(MANIFEST_CAP));
        assert_eq!(refusal(f.verify()), Refusal::EntryTooLarge);
        let mut f = Fixture::new();
        let mut sig = f.key.sign(&f.checksums_bytes(), &f.comment());
        sig.insert_str(0, &" ".repeat(SIGNATURE_CAP));
        f.signature = Some(Some(sig.into_bytes()));
        assert_eq!(refusal(f.verify()), Refusal::EntryTooLarge);
    }

    // ---- The manifest ------------------------------------------------------------------------

    /// Change the manifest (and re-sign over it), expecting `refusal`.
    fn refused_manifest(what: &str, want: Refusal, change: impl FnOnce(&mut Value)) {
        let f = with_manifest(change);
        assert_eq!(refusal(f.verify()), want, "{what}");
    }

    #[test]
    fn the_licence_must_be_the_agpl() {
        refused_manifest("MIT", Refusal::Licence, |m| m["licence"] = json!("MIT"));
        refused_manifest("lowercase", Refusal::Licence, |m| {
            m["licence"] = json!("agpl-3.0-or-later")
        });
        refused_manifest("-only", Refusal::Licence, |m| {
            m["licence"] = json!("AGPL-3.0-only")
        });
        let mut f = Fixture::new();
        f.files
            .insert(LICENCE_FILE.into(), b"MIT License\n".to_vec());
        assert_eq!(refusal(f.verify()), Refusal::Licence);
    }

    #[test]
    fn the_id_must_parse_and_fit_the_tier() {
        for (id, want) in [
            ("Atlas.Map", Refusal::Id),
            ("atlas.map;x", Refusal::Id),
            ("atlas.con", Refusal::Id),
            ("atlas", Refusal::Id),
        ] {
            refused_manifest(id, want, |m| m["id"] = json!(id));
        }
        // An atlas id signed as second party, and first party outside atlas.
        refused_manifest("second-party atlas", Refusal::Id, |m| {
            m["publisher"]["tier"] = json!("second")
        });
        refused_manifest("first-party acme", Refusal::Id, |m| {
            m["id"] = json!("acme.map")
        });
    }

    #[test]
    fn unknown_repeated_or_null_fields_are_refused() {
        refused_manifest("unknown key", Refusal::Manifest, |m| m["extra"] = json!(1));
        refused_manifest("unknown publisher key", Refusal::Manifest, |m| {
            m["publisher"]["email"] = json!("x@example.com")
        });
        refused_manifest("pinboard.widget (D16)", Refusal::Manifest, |m| {
            m["contributions"]["pinboard.widget"] = json!({"label": "x"})
        });
        refused_manifest("null icon", Refusal::Manifest, |m| m["icon"] = Value::Null);
        refused_manifest("null engines", Refusal::Manifest, |m| {
            m["engines"] = Value::Null
        });
        refused_manifest("missing entry", Refusal::Manifest, |m| {
            m.as_object_mut().unwrap().remove("entry");
        });
        refused_manifest("wrong type", Refusal::Manifest, |m| m["name"] = json!(1));
        // A key written twice: serde_json's derive refuses it; checked on the raw text.
        let text = serde_json::to_string(&good_manifest()).unwrap();
        let twice = text.replacen("\"name\":\"Map\"", "\"name\":\"Map\",\"name\":\"PM\"", 1);
        assert_ne!(text, twice);
        let files = BTreeMap::new();
        let e = parse_manifest(twice.as_bytes(), &files).unwrap_err();
        assert_eq!(e.refusal, Refusal::Manifest);
    }

    #[test]
    fn engines_is_absent_or_exactly_frame_true() {
        assert!(with_manifest(|m| {
            m.as_object_mut().unwrap().remove("engines");
        })
        .verify()
        .is_ok());
        for engines in [
            json!({"frame": false}),
            json!({"frame": true, "workers": true}),
            json!({}),
            json!({"python": true}),
            json!(true),
            json!("frame"),
        ] {
            refused_manifest(&engines.to_string(), Refusal::Manifest, |m| {
                m["engines"] = engines.clone()
            });
        }
    }

    #[test]
    fn names_and_descriptions_are_bounded_plain_text() {
        for name in [
            "",
            " Map",
            "Map ",
            "Map\nX",
            "Map\tX",
            "PM\u{202e}paM",
            "M\u{200b}ap",
            "Map\u{00a0}",
            "Ma\u{feff}p",
            "Map\u{e0041}",
            &"M".repeat(41),
        ] {
            refused_manifest(name, Refusal::Manifest, |m| m["name"] = json!(name));
        }
        assert!(with_manifest(|m| m["name"] = json!("Карта 地図 ✓"))
            .verify()
            .is_ok());
        refused_manifest("long description", Refusal::Manifest, |m| {
            m["description"] = json!("d".repeat(281))
        });
        refused_manifest("bidi description", Refusal::Manifest, |m| {
            m["description"] = json!("a\u{2066}b")
        });
        assert!(
            with_manifest(|m| m["description"] = json!("Line one.\nLine two."))
                .verify()
                .is_ok()
        );
    }

    #[test]
    fn version_and_api_range_are_strict() {
        // (An empty version never gets here: the trusted comment refuses an empty field.)
        for v in [
            "1.0",
            "v1.0.0",
            "01.0.0",
            "1.0.0+build.5",
            "1.0.0 ",
            "1.0.0-",
        ] {
            refused_manifest(v, Refusal::Manifest, |m| m["version"] = json!(v));
        }
        for (min, target) in [
            ("1.1", "1.0"),
            ("1", "1.0"),
            ("1.0.0", "1.0"),
            ("01.0", "1.0"),
            ("1.00", "1.0"),
            ("1.0", "x"),
            ("99999.0", "99999.0"),
        ] {
            refused_manifest(&format!("{min}-{target}"), Refusal::Manifest, |m| {
                m["apiVersion"] = json!({"min": min, "target": target})
            });
        }
    }

    #[test]
    fn the_publisher_is_shape_checked() {
        // Fields the trusted comment also carries must still be valid comment values here, so
        // the manifest's own check is the one that refuses them.
        for (key, value) in [
            ("author", json!(" Atlas")),
            ("author", json!("A\u{202e}B")),
            ("sourceRepo", json!("no-slash")),
            ("sourceRepo", json!("a/b/c")),
            ("sourceRepo", json!("-a/b")),
            ("sourceRepo", json!("a/..")),
            ("sourceRepo", json!("https://github.com/a/b")),
        ] {
            refused_manifest(&format!("{key}={value}"), Refusal::Manifest, |m| {
                m["publisher"][key] = value.clone()
            });
        }
        // A tier or commit the comment can't carry (it shape-checks both): signed with the good
        // one, written as something else.
        for (key, value) in [
            ("tier", json!("fourth")),
            ("commit", json!("abc")),
            ("commit", json!(COMMIT.to_uppercase())),
            ("commit", json!(format!("{COMMIT}0"))),
        ] {
            let mut f = with_manifest(|m| m["publisher"][key] = value.clone());
            f.comment = Some(comment_for(&good_manifest()));
            assert_eq!(refusal(f.verify()), Refusal::Manifest, "{key}={value}");
        }
    }

    #[test]
    fn permissions_are_known_unique_and_explained() {
        refused_manifest("unknown", Refusal::Manifest, |m| {
            m["permissions"][0]["id"] = json!("everything")
        });
        refused_manifest("twice", Refusal::Manifest, |m| {
            m["permissions"][1]["id"] = json!("storage")
        });
        refused_manifest("no why", Refusal::Manifest, |m| {
            m["permissions"][0]["why"] = json!("")
        });
        refused_manifest("nav.tab without ui.tab", Refusal::Manifest, |m| {
            m["permissions"].as_array_mut().unwrap().remove(1);
        });
        refused_manifest("data without storage", Refusal::Manifest, |m| {
            m["permissions"].as_array_mut().unwrap().remove(0);
        });
    }

    fn with_internet(m: &mut Value) {
        m["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "internet", "why": "Fetches tiles."}));
        m["internetHosts"] = json!(["tiles.example.com"]);
    }

    #[test]
    fn internet_hosts_are_exact_public_names_and_go_with_the_permission() {
        assert!(with_manifest(with_internet).verify().is_ok());
        for host in [
            "localhost",
            "pmmod.localhost",
            "tauri.localhost",
            "127.0.0.1",
            "10.0.0.1",
            "[::1]",
            "example.com:443",
            "*.example.com",
            "Example.com",
            "example",
            "-a.example.com",
            "a-.example.com",
            "example.com.",
            "https://example.com",
            "exa mple.com",
        ] {
            refused_manifest(host, Refusal::Manifest, |m| {
                with_internet(m);
                m["internetHosts"] = json!([host]);
            });
        }
        refused_manifest("twice", Refusal::Manifest, |m| {
            with_internet(m);
            m["internetHosts"] = json!(["a.example.com", "a.example.com"]);
        });
        refused_manifest("hosts without the permission", Refusal::Manifest, |m| {
            m["internetHosts"] = json!(["a.example.com"]);
        });
        refused_manifest("the permission without hosts", Refusal::Manifest, |m| {
            with_internet(m);
            m["internetHosts"] = json!([]);
        });
    }

    #[test]
    fn secrets_go_to_one_listed_host_in_one_ordinary_header() {
        let with_secret = |m: &mut Value, secret: Value| {
            with_internet(m);
            m["secrets"] = json!([secret]);
        };
        assert!(with_manifest(|m| with_secret(
            m,
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "Authorization"})
        ))
        .verify()
        .is_ok());
        for secret in [
            json!({"name": "apiKey", "host": "other.example.com", "header": "Authorization"}),
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "Host"}),
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "cookie"}),
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "Transfer-Encoding"}),
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "X Key"}),
            json!({"name": "apiKey", "host": "tiles.example.com", "header": "X-Key: 1"}),
            json!({"name": "Key", "host": "tiles.example.com", "header": "X-Key"}),
            json!({"name": "", "host": "tiles.example.com", "header": "X-Key"}),
            json!({"name": "k", "host": "tiles.example.com", "header": "X-Key", "value": "s3cret"}),
        ] {
            refused_manifest(&secret.to_string(), Refusal::Manifest, |m| {
                with_secret(m, secret.clone())
            });
        }
        refused_manifest("a secret with no internet", Refusal::Manifest, |m| {
            m["secrets"] = json!([{"name": "k", "host": "tiles.example.com", "header": "X-Key"}]);
        });
    }

    #[test]
    fn entry_icon_and_workers_name_listed_app_files() {
        for (what, change) in [
            ("entry not listed", json!({"page": "app/missing.html"})),
            ("entry outside app", json!({"page": "manifest.json"})),
            ("entry not html", json!({"page": "app/main.js"})),
            (
                "worker not listed",
                json!({"page": "app/index.html", "workers": ["app/nope.js"]}),
            ),
            (
                "worker not js",
                json!({"page": "app/index.html", "workers": ["app/icon.png"]}),
            ),
            (
                "worker twice",
                json!({"page": "app/index.html", "workers": ["app/main.js", "app/main.js"]}),
            ),
        ] {
            refused_manifest(what, Refusal::Manifest, |m| m["entry"] = change.clone());
        }
        refused_manifest("icon not listed", Refusal::Manifest, |m| {
            m["icon"] = json!("app/missing.png")
        });
        refused_manifest("icon not an image", Refusal::Manifest, |m| {
            m["icon"] = json!("app/main.js")
        });
    }

    #[test]
    fn contributions_data_and_requires_are_shape_checked() {
        refused_manifest("long tab label", Refusal::Manifest, |m| {
            m["contributions"]["nav.tab"]["label"] = json!("x".repeat(25))
        });
        refused_manifest("help id twice", Refusal::Manifest, |m| {
            let h = m["contributions"]["help"][0].clone();
            m["contributions"]["help"].as_array_mut().unwrap().push(h);
        });
        refused_manifest("help id shape", Refusal::Manifest, |m| {
            m["contributions"]["help"][0]["id"] = json!("Map Canvas")
        });
        refused_manifest("empty help body", Refusal::Manifest, |m| {
            m["contributions"]["help"][0]["body"] = json!("")
        });
        refused_manifest("data class", Refusal::Manifest, |m| {
            m["data"]["keys"][0]["class"] = json!("secret")
        });
        refused_manifest("data key twice", Refusal::Manifest, |m| {
            let k = m["data"]["keys"][0].clone();
            m["data"]["keys"].as_array_mut().unwrap().push(k);
        });
        refused_manifest("data key shape", Refusal::Manifest, |m| {
            m["data"]["keys"][0]["key"] = json!("Settings Key")
        });
        refused_manifest("projectRef to an undeclared key", Refusal::Manifest, |m| {
            m["data"]["projectRefs"] = json!([{"key": "board", "path": "$.items[*].project"}])
        });
        refused_manifest("projectRef path with a space", Refusal::Manifest, |m| {
            m["data"]["projectRefs"] = json!([{"key": "settings", "path": "$.a b"}])
        });
        assert!(with_manifest(|m| {
            m["data"]["projectRefs"] = json!([{"key": "settings", "path": "$.items[*].project"}])
        })
        .verify()
        .is_ok());
        refused_manifest("unknown room", Refusal::Manifest, |m| {
            m["requires"] = json!(["pinboard"])
        });
        refused_manifest("room twice", Refusal::Manifest, |m| {
            m["requires"] = json!(["calendar", "calendar"])
        });
        assert!(
            with_manifest(|m| m["requires"] = json!(["calendar", "localai"]))
                .verify()
                .is_ok()
        );
    }

    #[test]
    fn a_quoted_name_cant_fake_the_message_around_it() {
        let q = quote("x\", and it's fine\n");
        assert!(q.starts_with('"') && q.contains("\\n") && q.contains("\\\""));
        assert!(quote(&"a".repeat(200)).chars().count() < 90);
    }
}
