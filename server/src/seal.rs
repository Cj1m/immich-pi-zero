//! Originals stored in S3 are sealed on the Pi before the PUT.
//!
//! One AES-256-GCM key lives in `{data}/keys/aes-256.key`. Each object stores its own
//! 8-byte nonce prefix, and the chunk index fills the rest of the 12-byte nonce, so
//! the same key can seal every original. A Range request decrypts only the chunks it
//! needs. An object that does not start with `IMMQ` is left as plaintext so a file
//! uploaded before sealing still downloads.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aes_gcm::aead::Aead;
use aes_gcm::aead::KeyInit as AeadKeyInit;
use aes_gcm::aead::Payload;
use aes_gcm::{Aes256Gcm, Nonce};

const KEY_MAGIC: &[u8; 4] = b"IMAK";
const OBJECT_MAGIC: &[u8; 4] = b"IMMQ";
const KEY_VERSION: u8 = 1;
const VERSION: u8 = 2;
const AES_KEY_LEN: usize = 32;
const NONCE_PREFIX_LEN: usize = 8;
pub(crate) const HEADER_LEN: usize = 17 + NONCE_PREFIX_LEN;
pub(crate) const CHUNK_SIZE: u32 = 1024 * 1024;
const TAG_LEN: u64 = 16;
const KEY_FILE: &str = "aes-256.key";

#[derive(Debug)]
pub(crate) struct SealError(pub String);

impl std::fmt::Display for SealError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

struct KeyMaterial {
    aes_key: [u8; AES_KEY_LEN],
}

/// The Pi's AES-256 key. The bytes never leave the data directory.
#[derive(Clone)]
pub(crate) struct ObjectKey {
    inner: Arc<KeyMaterial>,
    path: PathBuf,
}

/// Parsed header plus the AES key for this object.
#[derive(Clone)]
pub(crate) struct Opened {
    pub plain_len: u64,
    pub chunk_size: u32,
    nonce_prefix: [u8; NONCE_PREFIX_LEN],
    aes_key: [u8; AES_KEY_LEN],
}

impl std::fmt::Debug for ObjectKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectKey")
            .field("path", &self.path)
            .finish()
    }
}

impl ObjectKey {
    /// Load the key, or create it mode `0600` when the file is absent.
    pub(crate) fn load_or_create(directory: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let path = directory.join(KEY_FILE);
        let material = if path.is_file() {
            load_material(&path)?
        } else {
            let created = create_material(&path)?;
            eprintln!("s3 objects are encrypted with {}", path.display());
            created
        };
        Ok(Self {
            inner: Arc::new(material),
            path,
        })
    }

    pub(crate) fn seal_file(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        self.seal_file_chunked(src, dst, CHUNK_SIZE)
    }

    fn seal_file_chunked(&self, src: &Path, dst: &Path, chunk_size: u32) -> std::io::Result<()> {
        if chunk_size == 0 || chunk_size > CHUNK_SIZE {
            return Err(std::io::Error::other("invalid seal chunk size"));
        }
        let mut input = File::open(src)?;
        let plain_len = input.metadata()?.len();
        let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
        getrandom::fill(&mut nonce_prefix)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut output = File::create(dst)?;
        output.write_all(OBJECT_MAGIC)?;
        output.write_all(&[VERSION])?;
        output.write_all(&plain_len.to_le_bytes())?;
        output.write_all(&chunk_size.to_le_bytes())?;
        output.write_all(&nonce_prefix)?;
        let mut index = 0u64;
        let mut offset = 0u64;
        let mut buffer = vec![0u8; chunk_size as usize];
        while offset < plain_len {
            let count = ((plain_len - offset) as usize).min(buffer.len());
            input.read_exact(&mut buffer[..count])?;
            let wire = encrypt_chunk(
                &self.inner.aes_key,
                &nonce_prefix,
                plain_len,
                index,
                &buffer[..count],
            )?;
            output.write_all(&wire)?;
            offset += count as u64;
            index += 1;
        }
        output.sync_all()?;
        Ok(())
    }

    pub(crate) fn open_header(&self, header: &[u8]) -> Result<Opened, SealError> {
        let (plain_len, chunk_size, nonce_prefix) = parse_header(header)?;
        Ok(Opened {
            plain_len,
            chunk_size,
            nonce_prefix,
            aes_key: self.inner.aes_key,
        })
    }
}

impl Opened {
    pub(crate) fn decrypt_chunk(&self, index: u64, wire: &[u8]) -> Result<Vec<u8>, SealError> {
        let plain_len = chunk_plain_len(self.plain_len, self.chunk_size, index)?;
        if wire.len() as u64 != plain_len + TAG_LEN {
            return Err(SealError("sealed chunk has the wrong length".to_string()));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.aes_key)
            .map_err(|_| SealError("AES-256 key was rejected".to_string()))?;
        let nonce_bytes = chunk_nonce(&self.nonce_prefix, index)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let aad = chunk_aad(self.plain_len, index);
        cipher
            .decrypt(
                nonce,
                Payload {
                    msg: wire,
                    aad: &aad,
                },
            )
            .map_err(|_| SealError("sealed chunk failed authentication".to_string()))
    }
}

pub(crate) fn looks_sealed(header: &[u8]) -> bool {
    header.len() >= 4 && header.starts_with(OBJECT_MAGIC)
}

pub(crate) fn sealed_len(plain_len: u64, chunk_size: u32) -> Option<u64> {
    let chunk = u64::from(chunk_size);
    if chunk == 0 {
        return None;
    }
    let chunks = if plain_len == 0 {
        0
    } else {
        plain_len.div_ceil(chunk)
    };
    let tags = chunks.checked_mul(TAG_LEN)?;
    (HEADER_LEN as u64)
        .checked_add(plain_len)?
        .checked_add(tags)
}

pub(crate) fn chunk_plain_len(
    plain_len: u64,
    chunk_size: u32,
    index: u64,
) -> Result<u64, SealError> {
    let chunk = u64::from(chunk_size);
    let start = index
        .checked_mul(chunk)
        .ok_or_else(|| SealError("chunk index overflow".to_string()))?;
    if start >= plain_len {
        return Err(SealError("chunk index is past the plaintext".to_string()));
    }
    Ok((plain_len - start).min(chunk))
}

pub(crate) fn chunk_offset(chunk_size: u32, index: u64) -> Result<u64, SealError> {
    let stride = u64::from(chunk_size) + TAG_LEN;
    let body = index
        .checked_mul(stride)
        .ok_or_else(|| SealError("chunk offset overflow".to_string()))?;
    (HEADER_LEN as u64)
        .checked_add(body)
        .ok_or_else(|| SealError("chunk offset overflow".to_string()))
}

/// Bytes of `plain` that fall inside the inclusive plaintext range.
pub(crate) fn overlap(
    index: u64,
    chunk_size: u32,
    range_start: u64,
    range_end: u64,
    plain: &[u8],
) -> &[u8] {
    let origin = index.saturating_mul(u64::from(chunk_size));
    let from = range_start.saturating_sub(origin) as usize;
    let to = (range_end.saturating_sub(origin) as usize).saturating_add(1);
    let from = from.min(plain.len());
    let to = to.min(plain.len());
    if to < from {
        &[]
    } else {
        &plain[from..to]
    }
}

fn parse_header(header: &[u8]) -> Result<(u64, u32, [u8; NONCE_PREFIX_LEN]), SealError> {
    if header.len() < HEADER_LEN || !header.starts_with(OBJECT_MAGIC) {
        return Err(SealError("object is not a sealed original".to_string()));
    }
    if header[4] != VERSION {
        return Err(SealError(
            "sealed object version is not supported".to_string(),
        ));
    }
    let plain_len = u64::from_le_bytes(header[5..13].try_into().expect("8 bytes"));
    let chunk_size = u32::from_le_bytes(header[13..17].try_into().expect("4 bytes"));
    if chunk_size == 0 || chunk_size > CHUNK_SIZE {
        return Err(SealError(
            "sealed object chunk size is not supported".to_string(),
        ));
    }
    let mut nonce_prefix = [0u8; NONCE_PREFIX_LEN];
    nonce_prefix.copy_from_slice(&header[17..HEADER_LEN]);
    Ok((plain_len, chunk_size, nonce_prefix))
}

fn chunk_nonce(prefix: &[u8; NONCE_PREFIX_LEN], index: u64) -> Result<[u8; 12], SealError> {
    let index = u32::try_from(index).map_err(|_| SealError("chunk index overflow".to_string()))?;
    let mut nonce = [0u8; 12];
    nonce[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    nonce[NONCE_PREFIX_LEN..].copy_from_slice(&index.to_le_bytes());
    Ok(nonce)
}

fn chunk_aad(plain_len: u64, index: u64) -> [u8; 16] {
    let mut aad = [0u8; 16];
    aad[..8].copy_from_slice(&plain_len.to_le_bytes());
    aad[8..].copy_from_slice(&index.to_le_bytes());
    aad
}

fn encrypt_chunk(
    aes_key: &[u8; AES_KEY_LEN],
    nonce_prefix: &[u8; NONCE_PREFIX_LEN],
    plain_len: u64,
    index: u64,
    plain: &[u8],
) -> std::io::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(aes_key)
        .map_err(|_| std::io::Error::other("AES-256 key was rejected"))?;
    let nonce_bytes =
        chunk_nonce(nonce_prefix, index).map_err(|error| std::io::Error::other(error.0))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let aad = chunk_aad(plain_len, index);
    cipher
        .encrypt(
            nonce,
            Payload {
                msg: plain,
                aad: &aad,
            },
        )
        .map_err(|_| std::io::Error::other("AES-GCM encrypt failed"))
}

fn load_material(path: &Path) -> std::io::Result<KeyMaterial> {
    let bytes = fs::read(path)?;
    if bytes.len() != 4 + 1 + AES_KEY_LEN || &bytes[..4] != KEY_MAGIC || bytes[4] != KEY_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("object key {} is damaged", path.display()),
        ));
    }
    let mut aes_key = [0u8; AES_KEY_LEN];
    aes_key.copy_from_slice(&bytes[5..]);
    Ok(KeyMaterial { aes_key })
}

fn create_material(path: &Path) -> std::io::Result<KeyMaterial> {
    let mut aes_key = [0u8; AES_KEY_LEN];
    getrandom::fill(&mut aes_key).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut body = Vec::with_capacity(4 + 1 + AES_KEY_LEN);
    body.extend_from_slice(KEY_MAGIC);
    body.push(KEY_VERSION);
    body.extend_from_slice(&aes_key);
    let temporary = path.with_extension("key.tmp");
    let _ = fs::remove_file(&temporary);
    {
        let mut file = create_private(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(KeyMaterial { aes_key })
}

fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(plain: &[u8], chunk: u32, start: u64, end: u64) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("immich-seal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let key = ObjectKey::load_or_create(&dir).unwrap();
        let src = dir.join("plain");
        let dst = dir.join("sealed");
        fs::write(&src, plain).unwrap();
        key.seal_file_chunked(&src, &dst, chunk).unwrap();
        let sealed = fs::read(&dst).unwrap();
        assert!(sealed.starts_with(b"IMMQ"));
        assert_eq!(sealed[4], VERSION);
        if plain.len() >= 8 {
            assert_ne!(&sealed[HEADER_LEN..HEADER_LEN + 8], &plain[..8]);
        }
        let reloaded = ObjectKey::load_or_create(&dir).unwrap();
        let opened = reloaded.open_header(&sealed[..HEADER_LEN]).unwrap();
        assert_eq!(opened.plain_len, plain.len() as u64);
        assert_eq!(
            sealed.len() as u64,
            sealed_len(opened.plain_len, chunk).unwrap()
        );
        let mut out = Vec::new();
        let first = start / u64::from(chunk);
        let last = end / u64::from(chunk);
        for index in first..=last {
            let offset = chunk_offset(chunk, index).unwrap() as usize;
            let wire_len = chunk_plain_len(opened.plain_len, chunk, index).unwrap() as usize + 16;
            let plain_chunk = opened
                .decrypt_chunk(index, &sealed[offset..offset + wire_len])
                .unwrap();
            out.extend_from_slice(overlap(index, chunk, start, end, &plain_chunk));
        }
        let _ = fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_sealed_object_roundtrips_each_range() {
        let plain = b"PLAINTEXT-MARKER-0123456789abcdefPLAINTEXT-MARKER-tail";
        let full = roundtrip(plain, 16, 0, plain.len() as u64 - 1);
        assert_eq!(full, plain);
        let middle = roundtrip(plain, 16, 10, 40);
        assert_eq!(middle, &plain[10..=40]);
        let tail = roundtrip(plain, 16, plain.len() as u64 - 5, plain.len() as u64 - 1);
        assert_eq!(tail, &plain[plain.len() - 5..]);
    }

    #[test]
    fn an_empty_object_seals_to_a_header() {
        let dir = std::env::temp_dir().join(format!("immich-seal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let key = ObjectKey::load_or_create(&dir).unwrap();
        let src = dir.join("plain");
        let dst = dir.join("sealed");
        fs::write(&src, b"").unwrap();
        key.seal_file(&src, &dst).unwrap();
        let sealed = fs::read(&dst).unwrap();
        assert_eq!(sealed.len(), HEADER_LEN);
        let opened = key.open_header(&sealed).unwrap();
        assert_eq!(opened.plain_len, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_chunk_fails_authentication() {
        let dir = std::env::temp_dir().join(format!("immich-seal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let key = ObjectKey::load_or_create(&dir).unwrap();
        let src = dir.join("plain");
        let dst = dir.join("sealed");
        fs::write(&src, b"PLAINTEXT-MARKER").unwrap();
        key.seal_file_chunked(&src, &dst, 16).unwrap();
        let mut sealed = fs::read(&dst).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        let opened = key.open_header(&sealed[..HEADER_LEN]).unwrap();
        let offset = chunk_offset(16, 0).unwrap() as usize;
        let error = opened.decrypt_chunk(0, &sealed[offset..]).unwrap_err();
        assert!(error.0.contains("authentication"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_key_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("immich-seal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(KEY_FILE), b"nope").unwrap();
        let error = ObjectKey::load_or_create(&dir).unwrap_err();
        assert!(error.to_string().contains("damaged"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_objects_use_different_nonce_prefixes() {
        let dir = std::env::temp_dir().join(format!("immich-seal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let key = ObjectKey::load_or_create(&dir).unwrap();
        let src = dir.join("plain");
        fs::write(&src, b"PLAINTEXT-MARKER").unwrap();
        let first = dir.join("one");
        let second = dir.join("two");
        key.seal_file_chunked(&src, &first, 16).unwrap();
        key.seal_file_chunked(&src, &second, 16).unwrap();
        let one = fs::read(&first).unwrap();
        let two = fs::read(&second).unwrap();
        assert_ne!(one[17..HEADER_LEN], two[17..HEADER_LEN]);
        let _ = fs::remove_dir_all(&dir);
    }
}
