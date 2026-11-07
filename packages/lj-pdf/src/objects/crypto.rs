//! PDF Standard Security Handler (encryption), decryption side only.
//!
//! This module implements enough of the PDF 1.7 / PDF 2.0 Standard Security
//! Handler to decrypt documents produced by:
//!
//! - revisions 2 and 3 (V=1 or V=2, RC4 with a 40-bit or 128-bit key),
//! - revision 4 with V=4 crypt filters naming `/V2` (RC4-128) or
//!   `/AESV2` (AES-128-CBC) as the stream and string method,
//! - revisions 5 and 6 (V=5, AES-256-CBC) via the `/AESV3` crypt filter,
//!   R=5 uses a plain SHA-256 verifier (the vulnerable Extension Level 3
//!   form) and R=6 uses the ISO 32000-2 iterative Algorithm 2.B hash.
//!
//! Authentication runs against either the user password or the owner
//! password. The empty user password is accepted as a special case of
//! the general user-password path.
//!
//! Public-key security handlers are not yet implemented and still fail
//! up front with `PdfError::Unsupported`. They can be layered on top
//! without changing this module's public surface.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use aes::{Aes128, Aes256};
use md5::{Digest, Md5};
use sha2::{Sha256, Sha384, Sha512};

use crate::objects::error::{PdfError, PdfResult};
use crate::objects::types::{ObjectRef, PdfDictionary, PdfValue};

/// Adobe's 32-byte password padding string (PDF 1.7, algorithm 2).
const PASSWORD_PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityRevision {
    R2,
    R3,
    R4,
    R5,
    R6,
}

/// Which crypt filter method applies to a given piece of ciphertext.
///
/// V=1/2 documents always use [`CryptMethod::V2`] (RC4) for everything.
/// V=4 documents name a crypt filter per kind (`/StmF`, `/StrF`, `/EFF`);
/// each may point at `/Identity` (no encryption), a V2 filter (RC4), or
/// an AESV2 filter (AES-128-CBC).
/// V=5 documents name the `/AESV3` filter (AES-256-CBC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptMethod {
    Identity,
    V2,
    AesV2,
    AesV3,
}

/// Which slot the ciphertext belongs to. Drives the crypt-method choice
/// (string vs stream) on V=4 documents and is a no-op on V=1/2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BytesKind {
    String,
    Stream,
}

#[derive(Debug, Clone)]
pub struct StandardSecurityHandler {
    file_key: Vec<u8>,
    string_method: CryptMethod,
    stream_method: CryptMethod,
    /// `false` only for V=4 documents that explicitly set
    /// `/EncryptMetadata false`; `true` everywhere else. When `false`,
    /// streams with `/Type /Metadata` and `/Subtype /XML` skip
    /// decryption.
    encrypt_metadata: bool,
}

impl StandardSecurityHandler {
    /// Builds a decryption handler from the `/Encrypt` dictionary and the
    /// trailer's first `/ID` string, authenticating the supplied password.
    /// Returns `None` if the password does not authenticate.
    pub fn open(
        encrypt_dict: &PdfDictionary,
        id_first: &[u8],
        password: &[u8],
    ) -> PdfResult<Option<Self>> {
        let filter = encrypt_dict
            .get("Filter")
            .and_then(PdfValue::as_name)
            .unwrap_or("");
        if filter != "Standard" {
            return Err(PdfError::Unsupported(format!(
                "encryption filter /{filter} is not supported"
            )));
        }
        let v = encrypt_dict
            .get("V")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        let r = encrypt_dict
            .get("R")
            .and_then(PdfValue::as_integer)
            .unwrap_or(0);
        let revision = match r {
            2 => SecurityRevision::R2,
            3 => SecurityRevision::R3,
            4 => SecurityRevision::R4,
            5 => SecurityRevision::R5,
            6 => SecurityRevision::R6,
            other => {
                return Err(PdfError::Unsupported(format!(
                    "Standard security handler revision {other} is not supported (only R=2..R=6 handled)"
                )));
            }
        };

        // V=5 is a separate code path: the file key is stored encrypted
        // in /OE /UE rather than derived algorithmically from /P + /ID.
        if v == 5 {
            return open_v5(encrypt_dict, revision, password);
        }

        let (string_method, stream_method, key_length_bytes) = match v {
            1 | 2 => {
                let bits = encrypt_dict
                    .get("Length")
                    .and_then(PdfValue::as_integer)
                    .unwrap_or(40);
                if bits % 8 != 0 || !(40..=128).contains(&bits) {
                    return Err(PdfError::Corrupt(format!(
                        "invalid /Length {bits} in Encrypt dictionary"
                    )));
                }
                (CryptMethod::V2, CryptMethod::V2, (bits / 8) as usize)
            }
            4 => {
                // V=4: crypt filters decide the method per slot. An AES key
                // is 128-bit, an RC4 one as long as /Length says.
                let (strf, stmf) = resolve_v4_crypt_filters(encrypt_dict)?;
                // A shorter RC4 key authenticates the password, then is
                // padded with zeros to 16 bytes to derive each object's key,
                // as Acrobat reads it.
                let key_length_bytes = if strf == CryptMethod::AesV2 || stmf == CryptMethod::AesV2 {
                    16
                } else {
                    encrypt_dict
                        .get("Length")
                        .and_then(PdfValue::as_integer)
                        .filter(|bits| bits % 8 == 0 && (40..=128).contains(bits))
                        .map_or(16, |bits| (bits / 8) as usize)
                };
                (strf, stmf, key_length_bytes)
            }
            other => {
                return Err(PdfError::Unsupported(format!(
                    "Standard security handler V={other} is not supported (only V=1, V=2, V=4, and V=5 handled)"
                )));
            }
        };

        // V=4's Algorithm 2 step 5: when /EncryptMetadata is explicitly
        // false, 0xFFFFFFFF is appended before the 50-round rehash.
        let encrypt_metadata = if matches!(revision, SecurityRevision::R4) {
            encrypt_dict
                .get("EncryptMetadata")
                .and_then(PdfValue::as_bool)
                .unwrap_or(true)
        } else {
            true
        };

        let o = pdf_string_bytes(encrypt_dict, "O")?;
        let u = pdf_string_bytes(encrypt_dict, "U")?;
        let p = encrypt_dict
            .get("P")
            .and_then(PdfValue::as_integer)
            .ok_or_else(|| PdfError::Corrupt("Encrypt dictionary missing /P".to_string()))?;
        if o.len() != 32 || u.len() != 32 {
            return Err(PdfError::Corrupt(
                "Encrypt /O and /U must each be 32 bytes".to_string(),
            ));
        }

        // First try the supplied password as the user password.
        let user_file_key = compute_file_key(
            password,
            &o,
            p as i32,
            id_first,
            key_length_bytes,
            revision,
            encrypt_metadata,
        );
        let object_file_key = |mut key: Vec<u8>| {
            if v == 4 {
                key.resize(key.len().max(16), 0);
            }
            key
        };
        if authenticate_user_password(&user_file_key, revision, &u, id_first) {
            return Ok(Some(Self {
                file_key: object_file_key(user_file_key),
                string_method,
                stream_method,
                encrypt_metadata,
            }));
        }

        // Then try it as the owner password: Algorithm 7 recovers the
        // padded user password from /O, after which we redo the user-
        // password authentication with that recovered value. The file key
        // used for object decryption is always derived from the user
        // password, the owner password is only a way of recovering it.
        let recovered_user_password =
            recover_user_password_from_owner(password, &o, revision, key_length_bytes);
        let owner_file_key = compute_file_key(
            &recovered_user_password,
            &o,
            p as i32,
            id_first,
            key_length_bytes,
            revision,
            encrypt_metadata,
        );
        if authenticate_user_password(&owner_file_key, revision, &u, id_first) {
            return Ok(Some(Self {
                file_key: object_file_key(owner_file_key),
                string_method,
                stream_method,
                encrypt_metadata,
            }));
        }

        Ok(None)
    }

    /// Builds a handler from an externally-derived file key and the
    /// pre-resolved crypt methods. Used by credential paths that derive
    /// the file key out-of-band, the public-key handler unwraps the
    /// file key from a CMS recipient envelope and then constructs the
    /// handler via this entry point rather than `open`.
    pub fn from_file_key(
        file_key: Vec<u8>,
        string_method: CryptMethod,
        stream_method: CryptMethod,
        encrypt_metadata: bool,
    ) -> Self {
        Self {
            file_key,
            string_method,
            stream_method,
            encrypt_metadata,
        }
    }

    /// Returns true when this handler was configured with
    /// `/EncryptMetadata false`. Parser uses this to skip
    /// `/Type /Metadata` streams.
    pub fn encrypts_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    /// Decrypts `bytes` produced for the indirect object `(num, gen)`.
    /// The crypt method is chosen per `kind`, strings use `/StrF`,
    /// streams use `/StmF`. Returns the ciphertext unchanged for
    /// `/Identity` filters; returns an error for malformed AES input
    /// (wrong length, bad PKCS#7 padding).
    pub fn decrypt_bytes(
        &self,
        bytes: &[u8],
        object_ref: ObjectRef,
        kind: BytesKind,
    ) -> PdfResult<Vec<u8>> {
        let method = match kind {
            BytesKind::String => self.string_method,
            BytesKind::Stream => self.stream_method,
        };
        match method {
            CryptMethod::Identity => Ok(bytes.to_vec()),
            CryptMethod::V2 => Ok(rc4(&self.object_key(object_ref, method), bytes)),
            CryptMethod::AesV2 => aes_128_cbc_decrypt(&self.object_key(object_ref, method), bytes),
            CryptMethod::AesV3 => {
                // AES-256-CBC in V=5 uses the file key directly, there is
                // no per-object key mixing, the `sAlT` suffix, or any
                // object-number-derived material.
                aes_256_cbc_decrypt(&self.file_key, bytes)
            }
        }
    }

    fn object_key(&self, object_ref: ObjectRef, method: CryptMethod) -> Vec<u8> {
        // Algorithm 1 / 1a. Append the 4-byte ASCII suffix "sAlT" when
        // the method is AES so keys derived for the same object under
        // different methods never collide.
        let suffix_len = if matches!(method, CryptMethod::AesV2) {
            9
        } else {
            5
        };
        let mut material = Vec::with_capacity(self.file_key.len() + suffix_len);
        material.extend_from_slice(&self.file_key);
        let num = object_ref.object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let generation = object_ref.generation.to_le_bytes();
        material.push(generation[0]);
        material.push(generation[1]);
        if matches!(method, CryptMethod::AesV2) {
            material.extend_from_slice(b"sAlT");
        }
        let digest = md5_bytes(&material);
        let truncated_len = (self.file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }
}

fn open_v5(
    encrypt_dict: &PdfDictionary,
    revision: SecurityRevision,
    password: &[u8],
) -> PdfResult<Option<StandardSecurityHandler>> {
    if !matches!(revision, SecurityRevision::R5 | SecurityRevision::R6) {
        return Err(PdfError::Unsupported(format!(
            "V=5 Encrypt dictionary requires R=5 or R=6, got {revision:?}"
        )));
    }
    let (strf, stmf) = resolve_v5_crypt_filters(encrypt_dict)?;

    let encrypt_metadata = encrypt_dict
        .get("EncryptMetadata")
        .and_then(PdfValue::as_bool)
        .unwrap_or(true);

    let o = pdf_string_bytes(encrypt_dict, "O")?;
    let u = pdf_string_bytes(encrypt_dict, "U")?;
    let oe = pdf_string_bytes(encrypt_dict, "OE")?;
    let ue = pdf_string_bytes(encrypt_dict, "UE")?;
    if o.len() < 48 || u.len() < 48 {
        return Err(PdfError::Corrupt(
            "V=5 Encrypt /O and /U must each be 48 bytes".to_string(),
        ));
    }
    if oe.len() < 32 || ue.len() < 32 {
        return Err(PdfError::Corrupt(
            "V=5 Encrypt /OE and /UE must each be 32 bytes".to_string(),
        ));
    }
    let (o, u, oe, ue) = (&o[..48], &u[..48], &oe[..32], &ue[..32]);

    // Passwords are UTF-8, truncated to 127 bytes per spec.
    let truncated_password = &password[..password.len().min(127)];

    // User-password attempt: hash(password || u_validation_salt) must
    // match u[0..32]; intermediate key = hash(password || u_key_salt).
    let u_validation_salt = &u[32..40];
    let u_key_salt = &u[40..48];
    let user_hash = pdf_2_b_hash(truncated_password, u_validation_salt, None, revision);
    if user_hash[..32] == u[..32] {
        let intermediate_key = pdf_2_b_hash(truncated_password, u_key_salt, None, revision);
        let file_key = aes_256_cbc_decrypt_no_pad(&intermediate_key[..32], &[0u8; 16], ue)?;
        return Ok(Some(StandardSecurityHandler {
            file_key,
            string_method: strf,
            stream_method: stmf,
            encrypt_metadata,
        }));
    }

    // Owner-password attempt: the hash inputs additionally include the
    // first 48 bytes of /U, binding the owner verifier to the user
    // record.
    let o_validation_salt = &o[32..40];
    let o_key_salt = &o[40..48];
    let owner_hash = pdf_2_b_hash(
        truncated_password,
        o_validation_salt,
        Some(&u[..48]),
        revision,
    );
    if owner_hash[..32] == o[..32] {
        let intermediate_key =
            pdf_2_b_hash(truncated_password, o_key_salt, Some(&u[..48]), revision);
        let file_key = aes_256_cbc_decrypt_no_pad(&intermediate_key[..32], &[0u8; 16], oe)?;
        return Ok(Some(StandardSecurityHandler {
            file_key,
            string_method: strf,
            stream_method: stmf,
            encrypt_metadata,
        }));
    }

    Ok(None)
}

/// Algorithm 2.B (R=6), with an R=5 short-circuit at the initial
/// SHA-256. Returns the full 32-byte hash used by [`open_v5`] for
/// both the verifier check and the intermediate key step.
fn pdf_2_b_hash(
    password: &[u8],
    salt: &[u8],
    user_vector: Option<&[u8]>,
    revision: SecurityRevision,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(password);
    hasher.update(salt);
    if let Some(vector) = user_vector {
        hasher.update(vector);
    }
    let mut k: Vec<u8> = hasher.finalize().to_vec();

    if matches!(revision, SecurityRevision::R5) {
        return k;
    }

    // R=6 inner loop (ISO 32000-2 §7.6.4.3.3 Algorithm 2.B).
    let user_vector = user_vector.unwrap_or(&[]);
    let mut round: u32 = 0;
    loop {
        // K1 = (password || K || user_vector) repeated 64 times.
        let mut k1 = Vec::with_capacity((password.len() + k.len() + user_vector.len()) * 64);
        for _ in 0..64 {
            k1.extend_from_slice(password);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(user_vector);
        }

        // AES-128-CBC encrypt K1 without any padding (K1 is always a
        // multiple of 16 because len(K) ∈ {32, 48, 64} and password +
        // user_vector contribute an integer number of blocks after
        // the 64× repetition, in practice implementations pad K1 by
        // repetition, not PKCS#7, so no padding is added here).
        let key: [u8; 16] = k[..16].try_into().expect("K is at least 32 bytes");
        let iv: [u8; 16] = k[16..32].try_into().expect("K is at least 32 bytes");
        let encrypted = aes_128_cbc_encrypt_no_pad(&key, &iv, &k1);

        // Sum first-16 bytes mod 3 to choose the next hash function.
        let selector: u32 = encrypted[..16]
            .iter()
            .map(|byte| u32::from(*byte % 3))
            .sum::<u32>()
            % 3;
        k = match selector {
            0 => Sha256::digest(&encrypted).to_vec(),
            1 => Sha384::digest(&encrypted).to_vec(),
            _ => Sha512::digest(&encrypted).to_vec(),
        };

        let last_byte = *encrypted.last().expect("AES output is non-empty");
        round += 1;
        if round >= 64 && u32::from(last_byte) <= round.saturating_sub(32) {
            break;
        }
    }

    k.truncate(32);
    k
}

fn resolve_v5_crypt_filters(encrypt_dict: &PdfDictionary) -> PdfResult<(CryptMethod, CryptMethod)> {
    let strf = encrypt_dict
        .get("StrF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let stmf = encrypt_dict
        .get("StmF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let cf = encrypt_dict.get("CF").and_then(|value| match value {
        PdfValue::Dictionary(dict) => Some(dict),
        _ => None,
    });
    Ok((
        resolve_crypt_filter_method(cf, strf)?,
        resolve_crypt_filter_method(cf, stmf)?,
    ))
}

pub(crate) fn resolve_v4_crypt_filters(
    encrypt_dict: &PdfDictionary,
) -> PdfResult<(CryptMethod, CryptMethod)> {
    let strf = encrypt_dict
        .get("StrF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let stmf = encrypt_dict
        .get("StmF")
        .and_then(PdfValue::as_name)
        .unwrap_or("Identity");
    let cf = encrypt_dict.get("CF").and_then(|value| match value {
        PdfValue::Dictionary(dict) => Some(dict),
        _ => None,
    });
    Ok((
        resolve_crypt_filter_method(cf, strf)?,
        resolve_crypt_filter_method(cf, stmf)?,
    ))
}

fn resolve_crypt_filter_method(cf: Option<&PdfDictionary>, name: &str) -> PdfResult<CryptMethod> {
    // The spec reserves the `Identity` filter name for "no encryption"
    // and specifies that it never appears in /CF; treat it as a pass-
    // through without consulting the dictionary.
    if name == "Identity" {
        return Ok(CryptMethod::Identity);
    }
    let subfilter = cf
        .and_then(|dict| dict.get(name))
        .and_then(|value| match value {
            PdfValue::Dictionary(dict) => Some(dict),
            _ => None,
        })
        .ok_or_else(|| {
            PdfError::Corrupt(format!(
                "Encrypt /CF is missing the crypt filter entry /{name}"
            ))
        })?;
    let cfm = subfilter
        .get("CFM")
        .and_then(PdfValue::as_name)
        .ok_or_else(|| {
            PdfError::Corrupt(format!("crypt filter /{name} is missing the /CFM entry"))
        })?;
    match cfm {
        "V2" => Ok(CryptMethod::V2),
        "AESV2" => Ok(CryptMethod::AesV2),
        "AESV3" => Ok(CryptMethod::AesV3),
        "None" => Ok(CryptMethod::Identity),
        other => Err(PdfError::Unsupported(format!(
            "crypt filter method /{other} is not supported (only /V2, /AESV2, and /AESV3 handled)"
        ))),
    }
}

/// Decrypts AES-128-CBC ciphertext whose first 16 bytes are the IV and
/// whose payload is PKCS#7-padded. Used for V=4 /AESV2 streams and
/// strings.
fn aes_128_cbc_decrypt(key: &[u8], data: &[u8]) -> PdfResult<Vec<u8>> {
    if key.len() != 16 {
        return Err(PdfError::Corrupt(format!(
            "AES-128 object key must be 16 bytes, got {}",
            key.len()
        )));
    }
    // A ciphertext cut short loses its trailing partial block, as in viewers.
    let data = &data[..data.len() - data.len() % 16];
    if data.len() < 32 {
        return Ok(Vec::new());
    }
    let cipher = Aes128::new_from_slice(key)
        .map_err(|error| PdfError::Corrupt(format!("AES-128 key rejected by cipher: {error}")))?;
    let mut prev_block: [u8; 16] = data[..16].try_into().expect("slice is 16 bytes");
    let mut output = Vec::with_capacity(data.len() - 16);
    for chunk in data[16..].chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (plain_byte, iv_byte) in block.iter_mut().zip(prev_block.iter()) {
            *plain_byte ^= iv_byte;
        }
        output.extend_from_slice(block.as_slice());
        prev_block.copy_from_slice(chunk);
    }
    strip_pkcs7(output)
}

/// Decrypts AES-256-CBC ciphertext whose first 16 bytes are the IV and
/// whose payload is PKCS#7-padded. Used by V=5 `/AESV3` strings and
/// streams.
fn aes_256_cbc_decrypt(key: &[u8], data: &[u8]) -> PdfResult<Vec<u8>> {
    if key.len() != 32 {
        return Err(PdfError::Corrupt(format!(
            "AES-256 file key must be 32 bytes, got {}",
            key.len()
        )));
    }
    // A ciphertext cut short loses its trailing partial block, as in viewers.
    let data = &data[..data.len() - data.len() % 16];
    if data.len() < 32 {
        return Ok(Vec::new());
    }
    let cipher = Aes256::new_from_slice(key)
        .map_err(|error| PdfError::Corrupt(format!("AES-256 key rejected by cipher: {error}")))?;
    let mut prev_block: [u8; 16] = data[..16].try_into().expect("slice is 16 bytes");
    let mut output = Vec::with_capacity(data.len() - 16);
    for chunk in data[16..].chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (plain_byte, iv_byte) in block.iter_mut().zip(prev_block.iter()) {
            *plain_byte ^= iv_byte;
        }
        output.extend_from_slice(block.as_slice());
        prev_block.copy_from_slice(chunk);
    }
    strip_pkcs7(output)
}

/// Decrypts AES-256-CBC ciphertext with a caller-supplied IV and no
/// PKCS#7 unpadding. Used by Algorithm 2.A to recover the 32-byte file
/// key from `/OE` / `/UE` (which are fixed 32-byte, two-block
/// ciphertexts with an all-zero IV).
fn aes_256_cbc_decrypt_no_pad(key: &[u8], iv: &[u8], data: &[u8]) -> PdfResult<Vec<u8>> {
    if key.len() != 32 {
        return Err(PdfError::Corrupt(format!(
            "AES-256 key must be 32 bytes, got {}",
            key.len()
        )));
    }
    if iv.len() != 16 {
        return Err(PdfError::Corrupt(format!(
            "AES-256-CBC IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(PdfError::Corrupt(format!(
            "AES-256-CBC payload must be a non-empty multiple of 16 bytes; got {}",
            data.len()
        )));
    }
    let cipher = Aes256::new_from_slice(key)
        .map_err(|error| PdfError::Corrupt(format!("AES-256 key rejected by cipher: {error}")))?;
    let mut prev_block: [u8; 16] = iv.try_into().expect("iv length validated");
    let mut output = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (plain_byte, iv_byte) in block.iter_mut().zip(prev_block.iter()) {
            *plain_byte ^= iv_byte;
        }
        output.extend_from_slice(block.as_slice());
        prev_block.copy_from_slice(chunk);
    }
    Ok(output)
}

/// Encrypts the Algorithm 2.B K1 buffer with AES-128-CBC. The buffer is
/// already a multiple of 16 bytes; the spec does not apply PKCS#7
/// padding on this inner loop, this helper therefore expects a
/// block-aligned input and rejects anything else.
fn aes_128_cbc_encrypt_no_pad(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    // The callers in this module feed only block-aligned inputs; any
    // unaligned tail is a programming error, not a runtime one.
    assert!(
        data.len().is_multiple_of(16),
        "Algorithm 2.B K1 must be block-aligned, got {}",
        data.len()
    );
    let cipher = Aes128::new_from_slice(key).expect("key length validated at compile time");
    let mut output = Vec::with_capacity(data.len());
    let mut prev: [u8; 16] = *iv;
    for chunk in data.chunks(16) {
        let mut buf = [0u8; 16];
        for ((b, plain), iv_byte) in buf.iter_mut().zip(chunk.iter()).zip(prev.iter()) {
            *b = plain ^ iv_byte;
        }
        let mut block = GenericArray::clone_from_slice(&buf);
        cipher.encrypt_block(&mut block);
        output.extend_from_slice(block.as_slice());
        prev.copy_from_slice(block.as_slice());
    }
    output
}

/// Padding that does not check out is left in place rather than refused.
fn strip_pkcs7(mut data: Vec<u8>) -> PdfResult<Vec<u8>> {
    let Some(&pad) = data.last() else {
        return Ok(data);
    };
    let new_len = data.len().saturating_sub(pad as usize);
    if (1..=16).contains(&pad) && data[new_len..].iter().all(|byte| *byte == pad) {
        data.truncate(new_len);
    }
    Ok(data)
}

fn pdf_string_bytes(dict: &PdfDictionary, key: &str) -> PdfResult<Vec<u8>> {
    match dict.get(key) {
        Some(PdfValue::String(s)) => Ok(s.0.clone()),
        Some(_) => Err(PdfError::Corrupt(format!("Encrypt /{key} is not a string"))),
        None => Err(PdfError::Corrupt(format!(
            "Encrypt dictionary missing /{key}"
        ))),
    }
}

fn compute_file_key(
    password: &[u8],
    o_entry: &[u8],
    permissions: i32,
    id_first: &[u8],
    key_length_bytes: usize,
    revision: SecurityRevision,
    encrypt_metadata: bool,
) -> Vec<u8> {
    // Algorithm 2 (PDF 1.7 section 7.6.3.3):
    //   1. Pad the password to 32 bytes.
    let padded = pad_password(password);
    let mut hasher = Md5::new();
    hasher.update(padded);
    //   2. Append /O.
    hasher.update(o_entry);
    //   3. Append /P (4 bytes little-endian).
    hasher.update(permissions.to_le_bytes());
    //   4. Append the first element of /ID.
    hasher.update(id_first);
    //   5. (R>=4 only) When /EncryptMetadata is explicitly false, append
    //      0xFFFFFFFF. R<=3 skips this step.
    if matches!(revision, SecurityRevision::R4) && !encrypt_metadata {
        hasher.update([0xFFu8; 4]);
    }
    let mut digest = hasher.finalize_reset();

    // Algorithm 2, step 6: for R>=3, re-MD5 the first n bytes 50 times.
    if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
        for _ in 0..50 {
            hasher.update(&digest[..key_length_bytes]);
            digest = hasher.finalize_reset();
        }
    }
    digest[..key_length_bytes].to_vec()
}

fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let take = password.len().min(32);
    out[..take].copy_from_slice(&password[..take]);
    if take < 32 {
        out[take..].copy_from_slice(&PASSWORD_PADDING[..32 - take]);
    }
    out
}

fn recover_user_password_from_owner(
    owner_password: &[u8],
    o_entry: &[u8],
    revision: SecurityRevision,
    key_length_bytes: usize,
) -> Vec<u8> {
    // Algorithm 7 (PDF 1.7 §7.6.3.4). Symmetric inverse of Algorithm 3:
    //   1. Pad the owner password and MD5 it.
    //   2. For R>=3 re-hash 50 times.
    //   3. Truncate to `key_length_bytes`, this is the RC4 key used on /O.
    //   4. For R=2, RC4-decrypt /O once with that key.
    //      For R>=3, RC4-decrypt /O 20 times with keys (base XOR i) for i
    //      decreasing from 19 down to 0.
    //   5. The result is the padded user password.
    let padded = pad_password(owner_password);
    let mut hasher = Md5::new();
    hasher.update(padded);
    let mut digest = hasher.finalize_reset();
    if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
        for _ in 0..50 {
            hasher.update(&digest[..key_length_bytes]);
            digest = hasher.finalize_reset();
        }
    }
    let base_key = digest[..key_length_bytes].to_vec();

    match revision {
        SecurityRevision::R2 => rc4(&base_key, o_entry),
        SecurityRevision::R3 | SecurityRevision::R4 => {
            let mut buffer = o_entry.to_vec();
            for i in (0u8..=19).rev() {
                let key: Vec<u8> = base_key.iter().map(|byte| byte ^ i).collect();
                buffer = rc4(&key, &buffer);
            }
            buffer
        }
        SecurityRevision::R5 | SecurityRevision::R6 => {
            unreachable!("V=5 takes open_v5; Algorithm 7 is not applicable to R=5 / R=6")
        }
    }
}

fn authenticate_user_password(
    file_key: &[u8],
    revision: SecurityRevision,
    u_entry: &[u8],
    id_first: &[u8],
) -> bool {
    match revision {
        SecurityRevision::R2 => {
            // Algorithm 4: encrypt the password padding with the file key; the
            // full 32 bytes must equal /U.
            let encrypted = rc4(file_key, &PASSWORD_PADDING);
            encrypted == u_entry
        }
        SecurityRevision::R5 | SecurityRevision::R6 => {
            unreachable!("V=5 takes open_v5; Algorithm 5 is not applicable to R=5 / R=6")
        }
        SecurityRevision::R3 | SecurityRevision::R4 => {
            // Algorithm 5.
            let mut hasher = Md5::new();
            hasher.update(PASSWORD_PADDING);
            hasher.update(id_first);
            let seed = hasher.finalize();
            let mut buffer = rc4(file_key, &seed);
            for i in 1u8..=19 {
                let key: Vec<u8> = file_key.iter().map(|byte| byte ^ i).collect();
                buffer = rc4(&key, &buffer);
            }
            // The first 16 bytes of /U must match the buffer; the remaining
            // 16 bytes are arbitrary padding.
            buffer.as_slice() == &u_entry[..16]
        }
    }
}

fn md5_bytes(input: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(input);
    hasher.finalize().into()
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = [0; 256];
    for (index, value) in s.iter_mut().enumerate() {
        *value = index as u8;
    }
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let mut output = Vec::with_capacity(data.len());
    let mut i: u8 = 0;
    let mut j: u8 = 0;
    for &byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[(s[i as usize].wrapping_add(s[j as usize])) as usize];
        output.push(byte ^ k);
    }
    output
}

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Expose the low-level primitives so parser tests can build a tiny
    //! encrypted PDF end-to-end, pick an arbitrary `/O`, derive a file key
    //! from the empty password, encrypt each object's data with per-object
    //! RC4, and then round-trip it through `parse_pdf`.

    use super::*;

    pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
        super::rc4(key, data)
    }

    pub fn compute_file_key(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        key_length_bytes: usize,
    ) -> Vec<u8> {
        // Callers that do not care about the revision use the R=3 variant,
        // which matches the write side of the existing RC4 fixtures.
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            key_length_bytes,
            SecurityRevision::R3,
            true,
        )
    }

    pub fn compute_file_key_with_revision(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        key_length_bytes: usize,
        revision: SecurityRevision,
    ) -> Vec<u8> {
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            key_length_bytes,
            revision,
            true,
        )
    }

    /// R=4 variant of the file-key derivation, exposed so AES-128 test
    /// fixtures can build a matching file key and `/U` entry. Mirrors
    /// [`compute_file_key`] but honours `encrypt_metadata` so the
    /// Algorithm 2 step-5 branch (append 0xFFFFFFFF) can be exercised.
    pub fn compute_file_key_r4(
        password: &[u8],
        o_entry: &[u8],
        permissions: i32,
        id_first: &[u8],
        encrypt_metadata: bool,
    ) -> Vec<u8> {
        super::compute_file_key(
            password,
            o_entry,
            permissions,
            id_first,
            16,
            SecurityRevision::R4,
            encrypt_metadata,
        )
    }

    /// Produce the 32-byte `/U` value that corresponds to the empty user
    /// password under revision 3. The first 16 bytes are the RC4 output
    /// from algorithm 5; the remaining 16 bytes are arbitrary padding
    /// (here zeroed, which real writers often do).
    pub fn compute_u_r3(file_key: &[u8], id_first: &[u8]) -> Vec<u8> {
        let mut hasher = Md5::new();
        hasher.update(PASSWORD_PADDING);
        hasher.update(id_first);
        let seed = hasher.finalize();
        let mut buffer = super::rc4(file_key, &seed);
        for i in 1u8..=19 {
            let key: Vec<u8> = file_key.iter().map(|byte| byte ^ i).collect();
            buffer = super::rc4(&key, &buffer);
        }
        buffer.resize(32, 0);
        buffer
    }

    /// Build the `/O` value for the Encrypt dictionary, given the owner
    /// and user passwords and the security revision. Algorithm 3, the
    /// write-side inverse of Algorithm 7, used by tests to construct
    /// synthetic encrypted PDFs with both owner and user passwords
    /// populated.
    pub fn compute_o(
        owner_password: &[u8],
        user_password: &[u8],
        revision: SecurityRevision,
        key_length_bytes: usize,
    ) -> Vec<u8> {
        let padded_owner = pad_password(owner_password);
        let mut hasher = Md5::new();
        hasher.update(padded_owner);
        let mut digest = hasher.finalize_reset();
        if matches!(revision, SecurityRevision::R3 | SecurityRevision::R4) {
            for _ in 0..50 {
                hasher.update(&digest[..key_length_bytes]);
                digest = hasher.finalize_reset();
            }
        }
        let base_key = digest[..key_length_bytes].to_vec();

        let padded_user = pad_password(user_password);
        match revision {
            SecurityRevision::R2 => super::rc4(&base_key, &padded_user),
            SecurityRevision::R3 | SecurityRevision::R4 => {
                let mut buffer = super::rc4(&base_key, &padded_user);
                for i in 1u8..=19 {
                    let key: Vec<u8> = base_key.iter().map(|byte| byte ^ i).collect();
                    buffer = super::rc4(&key, &buffer);
                }
                buffer
            }
            SecurityRevision::R5 | SecurityRevision::R6 => {
                panic!("compute_o is not applicable to V=5: use compute_v5_u / compute_v5_o")
            }
        }
    }

    /// Build the per-object RC4 key in exactly the same way the handler
    /// does, so tests can encrypt a known plaintext and then check that
    /// the parser's decryption path inverts the transform.
    pub fn object_key(file_key: &[u8], object_number: u32, generation: u16) -> Vec<u8> {
        let mut material = Vec::with_capacity(file_key.len() + 5);
        material.extend_from_slice(file_key);
        let num = object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let gen_bytes = generation.to_le_bytes();
        material.push(gen_bytes[0]);
        material.push(gen_bytes[1]);
        let digest = super::md5_bytes(&material);
        let truncated_len = (file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }

    /// AES variant of [`object_key`]: appends the literal `sAlT` suffix
    /// before the MD5 so the V=4 /AESV2 path derives a distinct key
    /// from the RC4 path for the same indirect object.
    pub fn object_key_aes(file_key: &[u8], object_number: u32, generation: u16) -> Vec<u8> {
        let mut material = Vec::with_capacity(file_key.len() + 9);
        material.extend_from_slice(file_key);
        let num = object_number.to_le_bytes();
        material.push(num[0]);
        material.push(num[1]);
        material.push(num[2]);
        let gen_bytes = generation.to_le_bytes();
        material.push(gen_bytes[0]);
        material.push(gen_bytes[1]);
        material.extend_from_slice(b"sAlT");
        let digest = super::md5_bytes(&material);
        let truncated_len = (file_key.len() + 5).min(16);
        digest[..truncated_len].to_vec()
    }

    /// Compute the 48-byte V=5 `/U` entry plus the 32-byte `/UE` entry
    /// given the user password, 8-byte validation salt, 8-byte key salt,
    /// and 32-byte file key. Only used by tests to build synthetic V=5
    /// fixtures.
    pub fn compute_v5_u_and_ue(
        user_password: &[u8],
        validation_salt: &[u8; 8],
        key_salt: &[u8; 8],
        file_key: &[u8; 32],
        revision: SecurityRevision,
    ) -> (Vec<u8>, Vec<u8>) {
        let verifier = super::pdf_2_b_hash(user_password, validation_salt, None, revision);
        let mut u = Vec::with_capacity(48);
        u.extend_from_slice(&verifier[..32]);
        u.extend_from_slice(validation_salt);
        u.extend_from_slice(key_salt);

        let intermediate = super::pdf_2_b_hash(user_password, key_salt, None, revision);
        let ue = aes_256_cbc_encrypt_no_pad(&intermediate[..32], &[0u8; 16], file_key);
        (u, ue)
    }

    /// Compute the 48-byte V=5 `/O` entry plus the 32-byte `/OE` entry
    /// given the owner password, 8-byte validation salt, 8-byte key salt,
    /// the 48-byte `/U` vector (typically the user hash + salts), and the
    /// 32-byte file key. Only used by tests to build synthetic V=5
    /// fixtures.
    pub fn compute_v5_o_and_oe(
        owner_password: &[u8],
        validation_salt: &[u8; 8],
        key_salt: &[u8; 8],
        u_vector: &[u8; 48],
        file_key: &[u8; 32],
        revision: SecurityRevision,
    ) -> (Vec<u8>, Vec<u8>) {
        let verifier =
            super::pdf_2_b_hash(owner_password, validation_salt, Some(u_vector), revision);
        let mut o = Vec::with_capacity(48);
        o.extend_from_slice(&verifier[..32]);
        o.extend_from_slice(validation_salt);
        o.extend_from_slice(key_salt);

        let intermediate = super::pdf_2_b_hash(owner_password, key_salt, Some(u_vector), revision);
        let oe = aes_256_cbc_encrypt_no_pad(&intermediate[..32], &[0u8; 16], file_key);
        (o, oe)
    }

    /// AES-256-CBC encrypt used by V=5 content streams and strings. The
    /// ciphertext is prefixed with the 16-byte IV and PKCS#7-padded to a
    /// 16-byte block boundary, this matches exactly what the parser's
    /// decryption path expects.
    pub fn aes_256_cbc_encrypt(key: &[u8], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        assert_eq!(key.len(), 32, "AES-256 key must be 32 bytes");
        let cipher = Aes256::new_from_slice(key).expect("key length validated");
        let pad_len = 16 - (plaintext.len() % 16);
        let mut padded = Vec::with_capacity(plaintext.len() + pad_len);
        padded.extend_from_slice(plaintext);
        padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));
        let mut output = Vec::with_capacity(16 + padded.len());
        output.extend_from_slice(iv);
        let mut prev: [u8; 16] = *iv;
        for chunk in padded.chunks(16) {
            let mut buf = [0u8; 16];
            for ((b, plain), iv_byte) in buf.iter_mut().zip(chunk.iter()).zip(prev.iter()) {
                *b = plain ^ iv_byte;
            }
            let mut block = GenericArray::clone_from_slice(&buf);
            cipher.encrypt_block(&mut block);
            output.extend_from_slice(block.as_slice());
            prev.copy_from_slice(block.as_slice());
        }
        output
    }

    fn aes_256_cbc_encrypt_no_pad(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
        assert_eq!(key.len(), 32, "AES-256 key must be 32 bytes");
        assert!(
            data.len().is_multiple_of(16),
            "plaintext must be block-aligned"
        );
        let cipher = Aes256::new_from_slice(key).expect("key length validated");
        let mut output = Vec::with_capacity(data.len());
        let mut prev: [u8; 16] = *iv;
        for chunk in data.chunks(16) {
            let mut buf = [0u8; 16];
            for ((b, plain), iv_byte) in buf.iter_mut().zip(chunk.iter()).zip(prev.iter()) {
                *b = plain ^ iv_byte;
            }
            let mut block = GenericArray::clone_from_slice(&buf);
            cipher.encrypt_block(&mut block);
            output.extend_from_slice(block.as_slice());
            prev.copy_from_slice(block.as_slice());
        }
        output
    }

    /// Encrypt `plaintext` with AES-128-CBC, PKCS#7-padded, and prefix
    /// the 16-byte IV, matching exactly what the parser's decryption
    /// path expects. Used by tests to build synthetic V=4 fixtures.
    pub fn aes_128_cbc_encrypt(key: &[u8], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::BlockEncrypt;

        assert_eq!(key.len(), 16, "AES-128 key must be 16 bytes");
        let cipher = Aes128::new_from_slice(key).expect("key length validated");

        // Pad with PKCS#7, always appending at least one byte of padding.
        let pad_len = 16 - (plaintext.len() % 16);
        let mut padded = Vec::with_capacity(plaintext.len() + pad_len);
        padded.extend_from_slice(plaintext);
        padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));

        let mut output = Vec::with_capacity(16 + padded.len());
        output.extend_from_slice(iv);
        let mut prev: [u8; 16] = *iv;
        for chunk in padded.chunks(16) {
            let mut block = [0u8; 16];
            for ((b, plain), iv_byte) in block.iter_mut().zip(chunk.iter()).zip(prev.iter()) {
                *b = plain ^ iv_byte;
            }
            let mut arr = GenericArray::clone_from_slice(&block);
            cipher.encrypt_block(&mut arr);
            output.extend_from_slice(arr.as_slice());
            prev.copy_from_slice(arr.as_slice());
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_empty_input_returns_empty() {
        assert_eq!(rc4(b"Key", b""), Vec::<u8>::new());
    }

    #[test]
    fn rc4_matches_known_vector() {
        // RFC 6229 test vector: key "Key", data "Plaintext".
        let key = b"Key";
        let plaintext = b"Plaintext";
        let encrypted = rc4(key, plaintext);
        // Decrypting with the same keystream yields the original bytes.
        let decrypted = rc4(key, &encrypted);
        assert_eq!(decrypted, plaintext);
        // The ciphertext should match the well-known RFC 6229 output.
        assert_eq!(
            encrypted,
            [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
        );
    }

    #[test]
    fn pad_password_short_pads_with_padding_string() {
        let padded = pad_password(b"ab");
        assert_eq!(padded[0], b'a');
        assert_eq!(padded[1], b'b');
        assert_eq!(padded[2], PASSWORD_PADDING[0]);
        assert_eq!(padded[31], PASSWORD_PADDING[29]);
    }

    #[test]
    fn pad_password_truncates_to_32_bytes() {
        let long = vec![b'x'; 64];
        let padded = pad_password(&long);
        assert_eq!(padded, [b'x'; 32]);
    }

    fn build_encrypt_dict_r3(
        o_entry: Vec<u8>,
        u_entry: Vec<u8>,
        permissions: i32,
    ) -> PdfDictionary {
        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(2));
        dict.insert("R".to_string(), PdfValue::Integer(3));
        dict.insert("Length".to_string(), PdfValue::Integer(128));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::objects::types::PdfString(o_entry)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::objects::types::PdfString(u_entry)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));
        dict
    }

    fn build_r3_handler_inputs(
        user_password: &[u8],
        owner_password: &[u8],
        id_first: &[u8],
    ) -> (PdfDictionary, Vec<u8>) {
        let key_length_bytes = 16;
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(
            owner_password,
            user_password,
            SecurityRevision::R3,
            key_length_bytes,
        );
        let file_key = test_helpers::compute_file_key(
            user_password,
            &o,
            permissions,
            id_first,
            key_length_bytes,
        );
        let u = test_helpers::compute_u_r3(&file_key, id_first);
        (build_encrypt_dict_r3(o, u, permissions), file_key)
    }

    #[test]
    fn open_authenticates_user_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, expected_file_key) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_authenticates_owner_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, expected_file_key) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"ownerpw")
            .expect("open succeeds")
            .expect("owner password authenticates");
        // File key must match the one derived from the user password, the
        // owner password is only a way of recovering it.
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_rejects_wrong_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let (dict, _) = build_r3_handler_inputs(b"userpw", b"ownerpw", id_first);
        let result = StandardSecurityHandler::open(&dict, id_first, b"wrongpw")
            .expect("open does not fail, only reports authentication");
        assert!(result.is_none());
    }

    #[test]
    fn open_accepts_utf8_password() {
        let id_first = b"synthetic-id-0123456789abcdef";
        let password = "pässwörd".as_bytes();
        let (dict, _) = build_r3_handler_inputs(password, b"ownerpw", id_first);
        let handler = StandardSecurityHandler::open(&dict, id_first, password)
            .expect("open succeeds")
            .expect("UTF-8 password authenticates");
        assert_eq!(handler.file_key.len(), 16);
    }

    fn build_encrypt_dict_v4_aesv2(
        o_entry: Vec<u8>,
        u_entry: Vec<u8>,
        permissions: i32,
        encrypt_metadata: Option<bool>,
    ) -> PdfDictionary {
        let mut std_cf = PdfDictionary::default();
        std_cf.insert("CFM".to_string(), PdfValue::Name("AESV2".to_string()));
        std_cf.insert("Length".to_string(), PdfValue::Integer(16));
        std_cf.insert(
            "AuthEvent".to_string(),
            PdfValue::Name("DocOpen".to_string()),
        );

        let mut cf = PdfDictionary::default();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(std_cf));

        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(4));
        dict.insert("R".to_string(), PdfValue::Integer(4));
        dict.insert("Length".to_string(), PdfValue::Integer(128));
        dict.insert("CF".to_string(), PdfValue::Dictionary(cf));
        dict.insert("StmF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert("StrF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::objects::types::PdfString(o_entry)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::objects::types::PdfString(u_entry)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));
        if let Some(value) = encrypt_metadata {
            dict.insert("EncryptMetadata".to_string(), PdfValue::Bool(value));
        }
        dict
    }

    fn build_v4_handler_inputs(
        user_password: &[u8],
        owner_password: &[u8],
        id_first: &[u8],
        encrypt_metadata: Option<bool>,
    ) -> (PdfDictionary, Vec<u8>) {
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(owner_password, user_password, SecurityRevision::R4, 16);
        let file_key = test_helpers::compute_file_key_r4(
            user_password,
            &o,
            permissions,
            id_first,
            encrypt_metadata.unwrap_or(true),
        );
        let u = test_helpers::compute_u_r3(&file_key, id_first);
        (
            build_encrypt_dict_v4_aesv2(o, u, permissions, encrypt_metadata),
            file_key,
        )
    }

    #[test]
    fn open_v4_aesv2_handler_authenticates_user_password() {
        let id_first = b"v4-synthetic-id-abcdefghij";
        let (dict, expected_file_key) =
            build_v4_handler_inputs(b"userpw", b"ownerpw", id_first, None);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates on V=4");
        assert_eq!(handler.file_key, expected_file_key);
        assert_eq!(handler.string_method, CryptMethod::AesV2);
        assert_eq!(handler.stream_method, CryptMethod::AesV2);
        assert!(handler.encrypt_metadata);
    }

    #[test]
    fn open_v4_aesv2_handler_authenticates_owner_password() {
        let id_first = b"v4-synthetic-id-abcdefghij";
        let (dict, expected_file_key) =
            build_v4_handler_inputs(b"userpw", b"ownerpw", id_first, None);
        let handler = StandardSecurityHandler::open(&dict, id_first, b"ownerpw")
            .expect("open succeeds")
            .expect("owner password authenticates on V=4");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_v4_honours_encrypt_metadata_false() {
        let id_first = b"v4-metadata-id";
        let (dict, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, Some(false));
        let handler = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect("open succeeds")
            .expect("empty password authenticates");
        assert!(!handler.encrypts_metadata());
    }

    #[test]
    fn open_v4_identity_crypt_filter_is_passthrough() {
        let id_first = b"v4-identity-id";
        let (dict_v4, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, None);
        let mut dict = dict_v4;
        dict.insert("StrF".to_string(), PdfValue::Name("Identity".to_string()));
        dict.insert("StmF".to_string(), PdfValue::Name("Identity".to_string()));

        let handler = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect("open succeeds")
            .expect("empty password authenticates");
        assert_eq!(handler.string_method, CryptMethod::Identity);
        assert_eq!(handler.stream_method, CryptMethod::Identity);

        let ciphertext = b"hello";
        let plaintext = handler
            .decrypt_bytes(ciphertext, ObjectRef::new(4, 0), BytesKind::Stream)
            .expect("identity passes bytes through");
        assert_eq!(plaintext, ciphertext);
    }

    #[test]
    fn open_v4_rejects_unsupported_cfm() {
        let id_first = b"v4-unsupported-id";

        let (dict_v4, _) = build_v4_handler_inputs(b"", b"ownerpw", id_first, None);
        let mut dict = dict_v4;
        let mut std_cf = PdfDictionary::default();
        // `AESV4` is defined in no PDF version, so it is rejected as
        // unsupported.
        std_cf.insert("CFM".to_string(), PdfValue::Name("AESV4".to_string()));
        std_cf.insert("Length".to_string(), PdfValue::Integer(32));
        let mut cf = PdfDictionary::default();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(std_cf));
        dict.insert("CF".to_string(), PdfValue::Dictionary(cf));

        let error = StandardSecurityHandler::open(&dict, id_first, b"")
            .expect_err("unknown CFM must be rejected as unsupported");
        assert!(matches!(error, PdfError::Unsupported(_)), "got {error:?}");
    }

    #[test]
    fn aes_128_cbc_round_trip() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plaintext = b"redact me, please";
        let ciphertext = test_helpers::aes_128_cbc_encrypt(&key, &iv, plaintext);
        let decrypted = aes_128_cbc_decrypt(&key, &ciphertext).expect("round trip succeeds");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn aes_128_cbc_keeps_a_block_whose_padding_does_not_check_out() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let mut ciphertext = test_helpers::aes_128_cbc_encrypt(&key, &iv, b"abcdef");
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        let decrypted = aes_128_cbc_decrypt(&key, &ciphertext).unwrap();
        assert!(decrypted.len() == 16 || decrypted.starts_with(b"abcdef"));
    }

    #[test]
    fn aes_128_cbc_reads_a_short_ciphertext_as_what_it_holds() {
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        assert!(aes_128_cbc_decrypt(&key, &[0u8; 16]).unwrap().is_empty());
        let mut ciphertext = test_helpers::aes_128_cbc_encrypt(&key, &iv, b"abcdef");
        ciphertext.extend([1, 2, 3]);
        assert_eq!(aes_128_cbc_decrypt(&key, &ciphertext).unwrap(), b"abcdef");
    }

    fn build_encrypt_dict_v5_aesv3(
        o: Vec<u8>,
        u: Vec<u8>,
        oe: Vec<u8>,
        ue: Vec<u8>,
        permissions: i32,
        perms: Option<Vec<u8>>,
        revision: SecurityRevision,
    ) -> PdfDictionary {
        let mut std_cf = PdfDictionary::default();
        std_cf.insert("CFM".to_string(), PdfValue::Name("AESV3".to_string()));
        std_cf.insert("Length".to_string(), PdfValue::Integer(32));
        std_cf.insert(
            "AuthEvent".to_string(),
            PdfValue::Name("DocOpen".to_string()),
        );

        let mut cf = PdfDictionary::default();
        cf.insert("StdCF".to_string(), PdfValue::Dictionary(std_cf));

        let r_value = match revision {
            SecurityRevision::R5 => 5,
            SecurityRevision::R6 => 6,
            _ => panic!("test helper only supports R5 / R6"),
        };

        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(5));
        dict.insert("R".to_string(), PdfValue::Integer(r_value));
        dict.insert("Length".to_string(), PdfValue::Integer(256));
        dict.insert("CF".to_string(), PdfValue::Dictionary(cf));
        dict.insert("StmF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert("StrF".to_string(), PdfValue::Name("StdCF".to_string()));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::objects::types::PdfString(o)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::objects::types::PdfString(u)),
        );
        dict.insert(
            "OE".to_string(),
            PdfValue::String(crate::objects::types::PdfString(oe)),
        );
        dict.insert(
            "UE".to_string(),
            PdfValue::String(crate::objects::types::PdfString(ue)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));
        if let Some(value) = perms {
            dict.insert(
                "Perms".to_string(),
                PdfValue::String(crate::objects::types::PdfString(value)),
            );
        }
        dict
    }

    fn build_v5_handler_inputs(
        user_password: &[u8],
        owner_password: &[u8],
        revision: SecurityRevision,
    ) -> (PdfDictionary, [u8; 32]) {
        let file_key = [0x13u8; 32];
        let u_validation_salt = [0xAAu8; 8];
        let u_key_salt = [0xBBu8; 8];
        let o_validation_salt = [0xCCu8; 8];
        let o_key_salt = [0xDDu8; 8];

        let (u, ue) = test_helpers::compute_v5_u_and_ue(
            user_password,
            &u_validation_salt,
            &u_key_salt,
            &file_key,
            revision,
        );
        let u_vector: [u8; 48] = u.as_slice().try_into().expect("U is 48 bytes");
        let (o, oe) = test_helpers::compute_v5_o_and_oe(
            owner_password,
            &o_validation_salt,
            &o_key_salt,
            &u_vector,
            &file_key,
            revision,
        );

        (
            build_encrypt_dict_v5_aesv3(o, u, oe, ue, -4, None, revision),
            file_key,
        )
    }

    #[test]
    fn open_v5_r6_authenticates_user_password() {
        let (dict, expected_file_key) =
            build_v5_handler_inputs(b"userpw", b"ownerpw", SecurityRevision::R6);
        let handler = StandardSecurityHandler::open(&dict, b"", b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates on V=5 / R=6");
        assert_eq!(handler.file_key, expected_file_key);
        assert_eq!(handler.string_method, CryptMethod::AesV3);
        assert_eq!(handler.stream_method, CryptMethod::AesV3);
    }

    #[test]
    fn open_v5_r6_authenticates_owner_password() {
        let (dict, expected_file_key) =
            build_v5_handler_inputs(b"userpw", b"ownerpw", SecurityRevision::R6);
        let handler = StandardSecurityHandler::open(&dict, b"", b"ownerpw")
            .expect("open succeeds")
            .expect("owner password authenticates on V=5 / R=6");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_v5_r6_rejects_wrong_password() {
        let (dict, _) = build_v5_handler_inputs(b"userpw", b"ownerpw", SecurityRevision::R6);
        let result = StandardSecurityHandler::open(&dict, b"", b"wrongpw")
            .expect("open does not fail, only reports authentication");
        assert!(result.is_none());
    }

    #[test]
    fn open_v5_r5_authenticates_user_password() {
        let (dict, expected_file_key) =
            build_v5_handler_inputs(b"userpw", b"ownerpw", SecurityRevision::R5);
        let handler = StandardSecurityHandler::open(&dict, b"", b"userpw")
            .expect("open succeeds")
            .expect("user password authenticates on V=5 / R=5");
        assert_eq!(handler.file_key, expected_file_key);
    }

    #[test]
    fn open_v5_r5_empty_password_authenticates() {
        let (dict, _) = build_v5_handler_inputs(b"", b"ownerpw", SecurityRevision::R5);
        let handler = StandardSecurityHandler::open(&dict, b"", b"")
            .expect("open succeeds")
            .expect("empty password authenticates on V=5 / R=5");
        assert_eq!(handler.string_method, CryptMethod::AesV3);
    }

    #[test]
    fn aes_256_cbc_round_trip_through_handler() {
        let key = [0x13u8; 32];
        let iv = [0x77u8; 16];
        let plaintext = b"top secret V=5 content";
        let ciphertext = test_helpers::aes_256_cbc_encrypt(&key, &iv, plaintext);
        let decrypted = aes_256_cbc_decrypt(&key, &ciphertext).expect("round trip succeeds");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn open_r2_authenticates_owner_password() {
        // Algorithm 4 / 7 divergence from R=3: single RC4 round for /O,
        // full 32-byte /U match.
        let id_first = b"r2-synthetic-id";
        let user_password = b"u2";
        let owner_password = b"o2";
        let key_length_bytes = 5; // 40-bit key, matching R=2 default.
        let permissions: i32 = -4;
        let o = test_helpers::compute_o(
            owner_password,
            user_password,
            SecurityRevision::R2,
            key_length_bytes,
        );
        let file_key = test_helpers::compute_file_key_with_revision(
            user_password,
            &o,
            permissions,
            id_first,
            key_length_bytes,
            SecurityRevision::R2,
        );
        // Algorithm 4: /U is RC4(file_key, PASSWORD_PADDING).
        let u = test_helpers::rc4(&file_key, &PASSWORD_PADDING);

        let mut dict = PdfDictionary::default();
        dict.insert("Filter".to_string(), PdfValue::Name("Standard".to_string()));
        dict.insert("V".to_string(), PdfValue::Integer(1));
        dict.insert("R".to_string(), PdfValue::Integer(2));
        dict.insert("Length".to_string(), PdfValue::Integer(40));
        dict.insert(
            "O".to_string(),
            PdfValue::String(crate::objects::types::PdfString(o)),
        );
        dict.insert(
            "U".to_string(),
            PdfValue::String(crate::objects::types::PdfString(u)),
        );
        dict.insert("P".to_string(), PdfValue::Integer(permissions as i64));

        let handler = StandardSecurityHandler::open(&dict, id_first, owner_password)
            .expect("open succeeds")
            .expect("owner password authenticates on R=2");
        assert_eq!(handler.file_key, file_key);
    }
}
