// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `.pmbackup` container: a cleartext header (magic, flags, KDF salt+params, the
//! STREAM nonce prefix) followed by the encrypted payload as a sequence of framed
//! AEAD chunks. Everything here is pure and I/O-generic (`impl Read`/`impl Write`), so
//! it unit-tests in memory with no keychain, DB, or filesystem.
//!
//! Payload framing, per chunk: `last_flag(1) | ct_len(u32 LE) | ciphertext`. The AEAD
//! itself (`aead::stream` BE32) is the real authority — it carries an internal 32-bit
//! counter and a last-block flag, so a dropped final chunk, a reordered chunk, or a
//! flipped `last_flag` all fail authentication. The explicit `last_flag` only tells the
//! reader which chunk to finalize; a mismatch can never be silently accepted.

use std::io::{self, Read, Write};

use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::Payload;
use chacha20poly1305::XChaCha20Poly1305;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::vault::kdf::KdfParams;

/// File magic (8 bytes) identifying a PM backup archive.
pub const MAGIC: &[u8; 8] = b"PMBACKUP";
/// The only container version this build writes/accepts.
pub const FORMAT_VERSION: u16 = 1;
/// Cipher identifier recorded in the header (informational + a guard against a future
/// scheme change opening under the wrong reader).
pub const CIPHER_ID: &str = "xchacha20poly1305-stream-be32";
/// Compression identifier recorded in the header.
pub const COMPRESSION_ID: &str = "zstd";
/// STREAM nonce-prefix length: XChaCha20's 24-byte nonce minus the 5 bytes BE32 spends
/// on its per-chunk counter (4) + last-block flag (1).
pub const NONCE_PREFIX_LEN: usize = 19;
/// Plaintext bytes per chunk (1 MiB) — keeps peak memory flat over a large DB.
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;

/// Flag bits recorded in the container header (non-secret provenance).
pub const FLAG_DB_KEY_EMBEDDED: u16 = 1 << 0;
pub const FLAG_SOURCE_MD_ENCRYPTED: u16 = 1 << 1;
pub const FLAG_SOURCE_KEY_MODE_PASSPHRASE: u16 = 1 << 2;

/// Sanity caps so a malformed/hostile archive can't drive an unbounded allocation.
const MAX_HEADER_LEN: usize = 64 * 1024;
/// Hard ceiling on the header-declared `chunk_size`. The frame-length guard is derived
/// from this, so without a cap a hostile header could authorize a multi-GiB per-frame
/// allocation before any authentication runs. 16 MiB is far above our 1 MiB default.
const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;
/// A ciphertext frame is at most one plaintext chunk + the AEAD tag + slack; anything
/// larger is a corrupt/hostile length prefix.
const FRAME_SLACK: usize = 4096;

/// The cleartext container header. Non-secret by construction — it holds only what a
/// restore needs to derive the key (salt + Argon2id params) and frame the stream. It
/// never carries the derived key or the passphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub format_version: u16,
    pub cipher: String,
    #[serde(flatten)]
    pub kdf: KdfBlock,
    pub stream_nonce_prefix_b64: String,
    pub chunk_size: usize,
    pub compression: String,
    pub created_at: String,
}

/// Argon2id params + salt (both non-secret), serialized flat into the header — mirrors
/// the vault's own `KdfBlock` shape so the two are recognisably siblings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfBlock {
    #[serde(flatten)]
    pub params: KdfParams,
    pub kdf_salt_b64: String,
}

/// Write the cleartext container header and return the exact JSON bytes, so the caller
/// can bind them to the payload as AAD (`aad(flags, &bytes)`).
pub fn write_header<W: Write>(w: &mut W, flags: u16, header: &Header) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(header)
        .map_err(|e| Error::Other(format!("could not encode backup header: {e}")))?;
    if json.len() > MAX_HEADER_LEN {
        return Err(Error::Other("backup header unexpectedly large".into()));
    }
    w.write_all(MAGIC)?;
    w.write_all(&FORMAT_VERSION.to_le_bytes())?;
    w.write_all(&flags.to_le_bytes())?;
    w.write_all(&(json.len() as u32).to_le_bytes())?;
    w.write_all(&json)?;
    Ok(json)
}

/// Parse and validate the cleartext header, returning the flags, the exact header JSON
/// bytes (for AAD), and the decoded [`Header`]. Reads nothing past the header, so an
/// unknown version fails before any crypto.
pub fn read_header<R: Read>(r: &mut R) -> Result<(u16, Vec<u8>, Header)> {
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)
        .map_err(|_| Error::Other("this file is not a PM backup".into()))?;
    if &magic != MAGIC {
        return Err(Error::Other(
            "this file is not a PM backup (bad signature)".into(),
        ));
    }
    let mut ver = [0u8; 2];
    r.read_exact(&mut ver)?;
    let version = u16::from_le_bytes(ver);
    if version != FORMAT_VERSION {
        return Err(Error::Other(format!(
            "this backup was written by a newer version of the app (format {version}); update to restore it"
        )));
    }
    let mut flags = [0u8; 2];
    r.read_exact(&mut flags)?;
    let flags = u16::from_le_bytes(flags);
    let mut hl = [0u8; 4];
    r.read_exact(&mut hl)?;
    let header_len = u32::from_le_bytes(hl) as usize;
    if header_len > MAX_HEADER_LEN {
        return Err(Error::Other(
            "corrupt backup header (length out of range)".into(),
        ));
    }
    let mut json = vec![0u8; header_len];
    r.read_exact(&mut json)?;
    let header: Header = serde_json::from_slice(&json)
        .map_err(|e| Error::Other(format!("corrupt backup header: {e}")))?;
    // L-4: the format version is recorded twice — the binary preamble (checked above) and the JSON
    // copy. Assert they agree so a header body claiming a different version can't slip past the reader.
    if header.format_version != version {
        return Err(Error::Other(format!(
            "corrupt backup header (format version mismatch: outer {version}, inner {})",
            header.format_version
        )));
    }
    // Bound the (attacker-controlled) chunk size before it feeds the frame allocation cap.
    if header.chunk_size == 0 || header.chunk_size > MAX_CHUNK_SIZE {
        return Err(Error::Other(
            "corrupt backup header (chunk size out of range)".into(),
        ));
    }
    Ok((flags, json, header))
}

/// Additional authenticated data binding the WHOLE cleartext preamble to the encrypted
/// payload: `blake3(MAGIC ‖ FORMAT_VERSION ‖ flags ‖ header_json)`. Covering the binary
/// preamble — not just the JSON — makes the format version and the provenance flags
/// (`FLAG_*`) tamper-evident too (L-4): any change to them alters the AAD and so fails
/// every chunk's authentication, rather than being silently accepted on restore.
pub fn aad(flags: u16, header_json: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(MAGIC);
    h.update(&FORMAT_VERSION.to_le_bytes());
    h.update(&flags.to_le_bytes());
    h.update(header_json);
    *h.finalize().as_bytes()
}

// ---- chunked STREAM writer/reader ---------------------------------------------------

/// Streaming AEAD writer: buffers plaintext into `chunk_size` blocks and seals each as a
/// framed `aead::stream` chunk. Implements [`Write`] so a `zstd::Encoder` can wrap it;
/// call [`finish`](Self::finish) to seal the final (possibly empty) block and recover
/// the inner writer. Peak memory is ~one chunk regardless of total size.
pub struct ChunkedAeadWriter<W: Write> {
    inner: W,
    enc: Option<EncryptorBE32<XChaCha20Poly1305>>,
    aad: [u8; 32],
    chunk_size: usize,
    pending: Vec<u8>,
}

impl<W: Write> ChunkedAeadWriter<W> {
    /// `nonce_prefix` must be [`NONCE_PREFIX_LEN`] bytes (validated).
    pub fn new(
        inner: W,
        cipher: XChaCha20Poly1305,
        nonce_prefix: &[u8],
        aad: [u8; 32],
        chunk_size: usize,
    ) -> Result<Self> {
        if nonce_prefix.len() != NONCE_PREFIX_LEN {
            return Err(Error::Other(
                "backup nonce prefix has the wrong length".into(),
            ));
        }
        let nonce = GenericArray::from_slice(nonce_prefix);
        let enc = EncryptorBE32::from_aead(cipher, nonce);
        Ok(Self {
            inner,
            enc: Some(enc),
            aad,
            chunk_size: chunk_size.max(1),
            pending: Vec::with_capacity(chunk_size),
        })
    }

    fn seal(&mut self, plaintext: &[u8], last: bool) -> io::Result<()> {
        let ct = if last {
            let enc = self
                .enc
                .take()
                .ok_or_else(|| io::Error::other("backup stream already finished"))?;
            enc.encrypt_last(Payload {
                msg: plaintext,
                aad: &self.aad,
            })
        } else {
            let enc = self
                .enc
                .as_mut()
                .ok_or_else(|| io::Error::other("backup stream already finished"))?;
            enc.encrypt_next(Payload {
                msg: plaintext,
                aad: &self.aad,
            })
        }
        .map_err(|_| io::Error::other("backup encryption failed"))?;
        self.inner.write_all(&[u8::from(last)])?;
        self.inner.write_all(&(ct.len() as u32).to_le_bytes())?;
        self.inner.write_all(&ct)?;
        Ok(())
    }

    /// Seal the trailing bytes as the final chunk and return the inner writer. Must be
    /// called exactly once; not calling it leaves an unterminated (unrestorable) stream.
    pub fn finish(mut self) -> io::Result<W> {
        let pending = std::mem::take(&mut self.pending);
        self.seal(&pending, true)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: Write> Write for ChunkedAeadWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while self.pending.len() >= self.chunk_size {
            let chunk: Vec<u8> = self.pending.drain(..self.chunk_size).collect();
            self.seal(&chunk, false)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Streaming AEAD reader: the inverse of [`ChunkedAeadWriter`]. Implements [`Read`] so a
/// `zstd::Decoder` can wrap it. A wrong passphrase (first chunk fails), a truncated
/// stream (EOF before the last-flagged chunk), tampering, or reordering all surface as
/// an `io::Error` here rather than yielding wrong plaintext.
pub struct ChunkedAeadReader<R: Read> {
    inner: R,
    dec: Option<DecryptorBE32<XChaCha20Poly1305>>,
    aad: [u8; 32],
    max_frame: usize,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> ChunkedAeadReader<R> {
    pub fn new(
        inner: R,
        cipher: XChaCha20Poly1305,
        nonce_prefix: &[u8],
        aad: [u8; 32],
        chunk_size: usize,
    ) -> Result<Self> {
        if nonce_prefix.len() != NONCE_PREFIX_LEN {
            return Err(Error::Other(
                "backup nonce prefix has the wrong length".into(),
            ));
        }
        let nonce = GenericArray::from_slice(nonce_prefix);
        let dec = DecryptorBE32::from_aead(cipher, nonce);
        Ok(Self {
            inner,
            dec: Some(dec),
            aad,
            max_frame: chunk_size.saturating_add(FRAME_SLACK),
            plain: Vec::new(),
            pos: 0,
            done: false,
        })
    }

    /// Read and decrypt the next frame into `self.plain`. Returns `Ok(false)` only on a
    /// clean end (which, before a last-flagged chunk, is a truncation error).
    fn fill(&mut self) -> io::Result<bool> {
        let mut flag = [0u8; 1];
        if !read_exact_or_eof(&mut self.inner, &mut flag)? {
            // Clean EOF, but we never saw the final (last-flagged) chunk.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the backup is incomplete or corrupted (the archive was truncated)",
            ));
        }
        let last = flag[0] == 1;
        let mut lenb = [0u8; 4];
        self.inner.read_exact(&mut lenb)?;
        let len = u32::from_le_bytes(lenb) as usize;
        if len > self.max_frame {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "corrupt backup (chunk length out of range)",
            ));
        }
        let mut ct = vec![0u8; len];
        self.inner.read_exact(&mut ct)?;
        let pt = if last {
            let dec = self
                .dec
                .take()
                .ok_or_else(|| io::Error::other("backup stream already finished"))?;
            dec.decrypt_last(Payload {
                msg: &ct,
                aad: &self.aad,
            })
        } else {
            let dec = self
                .dec
                .as_mut()
                .ok_or_else(|| io::Error::other("backup stream already finished"))?;
            dec.decrypt_next(Payload {
                msg: &ct,
                aad: &self.aad,
            })
        }
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "could not decrypt the backup — wrong passphrase, or the archive was tampered with",
            )
        })?;
        self.plain = pt;
        self.pos = 0;
        self.done = last;
        Ok(true)
    }
}

impl<R: Read> Read for ChunkedAeadReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.plain.len() {
                let n = std::cmp::min(buf.len(), self.plain.len() - self.pos);
                buf[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.done {
                return Ok(0);
            }
            self.fill()?;
        }
    }
}

/// Fill `buf` completely; return `Ok(false)` for a *clean* EOF at a frame boundary (zero
/// bytes read), `Ok(true)` on a full read, or an error on a partial read.
fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut read = 0;
    while read < buf.len() {
        match r.read(&mut buf[read..]) {
            Ok(0) => {
                if read == 0 {
                    return Ok(false);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unexpected end of backup archive",
                ));
            }
            Ok(n) => read += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chacha20poly1305::{Key, KeyInit};

    fn params() -> KdfParams {
        KdfParams {
            algorithm: "argon2id".to_string(),
            version: 0x13,
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            key_len: 32,
        }
    }

    fn sample_header() -> Header {
        Header {
            format_version: FORMAT_VERSION,
            cipher: CIPHER_ID.to_string(),
            kdf: KdfBlock {
                params: params(),
                kdf_salt_b64: "AAAAAAAAAAAAAAAAAAAAAA==".to_string(),
            },
            stream_nonce_prefix_b64: "bm9uY2UtcHJlZml4LTE5Yg==".to_string(),
            chunk_size: 16,
            compression: COMPRESSION_ID.to_string(),
            created_at: "2026-07-02T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn header_round_trips_and_returns_exact_bytes() {
        let h = sample_header();
        let mut buf = Vec::new();
        let written_json = write_header(&mut buf, FLAG_DB_KEY_EMBEDDED, &h).unwrap();
        let (flags, json, back) = read_header(&mut &buf[..]).unwrap();
        assert_eq!(flags, FLAG_DB_KEY_EMBEDDED);
        assert_eq!(json, written_json);
        assert_eq!(back, h);
        // The AAD is a pure function of the flags + header bytes.
        assert_eq!(aad(flags, &json), aad(FLAG_DB_KEY_EMBEDDED, &written_json));
        // L-4: the flags are now bound into the AAD, so tampering with them changes it.
        assert_ne!(
            aad(flags, &json),
            aad(flags ^ FLAG_SOURCE_MD_ENCRYPTED, &json)
        );
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        assert!(read_header(&mut &b"not-a-backup-file-at-all"[..]).is_err());
        // Correct magic, unsupported (future) version.
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&999u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        assert!(read_header(&mut &buf[..]).is_err());
    }

    fn cipher_from(key: [u8; 32]) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&key))
    }

    fn seal(
        key: [u8; 32],
        aad_bytes: [u8; 32],
        prefix: &[u8],
        chunk: usize,
        data: &[u8],
    ) -> Vec<u8> {
        let mut w =
            ChunkedAeadWriter::new(Vec::new(), cipher_from(key), prefix, aad_bytes, chunk).unwrap();
        w.write_all(data).unwrap();
        w.finish().unwrap()
    }

    fn make_reader(
        key: [u8; 32],
        aad_bytes: [u8; 32],
        prefix: &[u8],
        chunk: usize,
        sealed: &[u8],
    ) -> ChunkedAeadReader<std::io::Cursor<Vec<u8>>> {
        ChunkedAeadReader::new(
            std::io::Cursor::new(sealed.to_vec()),
            cipher_from(key),
            prefix,
            aad_bytes,
            chunk,
        )
        .unwrap()
    }

    const PREFIX: &[u8; NONCE_PREFIX_LEN] = b"nonce-prefix-19byte";

    #[test]
    fn stream_round_trips_across_chunk_boundaries() {
        let key = [3u8; 32];
        let aad_bytes = [9u8; 32];
        // Data larger than several chunks, plus non-multiple tail.
        let data: Vec<u8> = (0..(16 * 3 + 5)).map(|i| (i % 251) as u8).collect();
        let sealed = seal(key, aad_bytes, PREFIX, 16, &data);
        let mut r = make_reader(key, aad_bytes, PREFIX, 16, &sealed);
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn empty_payload_round_trips() {
        let key = [1u8; 32];
        let aad_bytes = [2u8; 32];
        let sealed = seal(key, aad_bytes, PREFIX, 16, b"");
        let mut r = make_reader(key, aad_bytes, PREFIX, 16, &sealed);
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn wrong_key_fails() {
        let aad_bytes = [9u8; 32];
        let data = b"the quick brown fox jumps over the lazy dog";
        let sealed = seal([3u8; 32], aad_bytes, PREFIX, 16, data);
        let mut r = make_reader([4u8; 32], aad_bytes, PREFIX, 16, &sealed);
        let mut out = Vec::new();
        assert!(r.read_to_end(&mut out).is_err());
    }

    #[test]
    fn wrong_aad_fails() {
        let data = b"header-bound payload";
        let sealed = seal([3u8; 32], [9u8; 32], PREFIX, 16, data);
        // A different AAD (a swapped header) must not authenticate.
        let mut r = make_reader([3u8; 32], [8u8; 32], PREFIX, 16, &sealed);
        let mut out = Vec::new();
        assert!(r.read_to_end(&mut out).is_err());
    }

    #[test]
    fn truncation_is_detected() {
        let data: Vec<u8> = (0..64).map(|i| i as u8).collect();
        let sealed = seal([3u8; 32], [9u8; 32], PREFIX, 16, &data);
        // Drop the trailing bytes (the final last-flagged chunk).
        let truncated = &sealed[..sealed.len() / 2];
        let mut r = make_reader([3u8; 32], [9u8; 32], PREFIX, 16, truncated);
        let mut out = Vec::new();
        assert!(r.read_to_end(&mut out).is_err());
    }

    #[test]
    fn tamper_in_a_chunk_fails() {
        let data: Vec<u8> = (0..48).map(|i| i as u8).collect();
        let mut sealed = seal([3u8; 32], [9u8; 32], PREFIX, 16, &data);
        // Flip a byte in the first ciphertext frame (past the 1+4 framing header).
        sealed[10] ^= 0x01;
        let mut r = make_reader([3u8; 32], [9u8; 32], PREFIX, 16, &sealed);
        let mut out = Vec::new();
        assert!(r.read_to_end(&mut out).is_err());
    }
}
