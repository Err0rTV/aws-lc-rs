// Copyright 2018 Brian Smith.
// SPDX-License-Identifier: ISC
// Modifications copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0 OR ISC

//! Block and Stream Cipher for Encryption and Decryption.
//!
//! # 🛑 Read Before Using
//!
//! This module provides access to block and stream cipher algorithms.
//! The modes provided here only provide confidentiality, but **do not**
//! provide integrity or authentication verification of ciphertext.
//!
//! These algorithms are provided solely for applications requring them
//! in order to maintain backwards compatability in legacy applications.
//!
//! If you are developing new applications requring data encryption see
//! the algorithms provided in [`aead`](crate::aead).
//!
//! # Examples
//! ```
//! use aws_lc_rs::cipher::{UnboundCipherKey, DecryptingKey, EncryptingKey, AES_128_CTR};
//!
//! let mut plaintext = Vec::from("This is a secret message!");
//!
//! let key_bytes: &[u8] = &[
//!     0xff, 0x0b, 0xe5, 0x84, 0x64, 0x0b, 0x00, 0xc8, 0x90, 0x7a, 0x4b, 0xbf, 0x82, 0x7c, 0xb6,
//!     0xd1,
//! ];
//!
//! let key = UnboundCipherKey::new(&AES_128_CTR, key_bytes).unwrap();
//! let encrypting_key = EncryptingKey::new(key).unwrap();
//! let iv = encrypting_key.encrypt(&mut plaintext).unwrap();
//!
//! let key = UnboundCipherKey::new(&AES_128_CTR, key_bytes).unwrap();
//! let decrypting_key = DecryptingKey::new(key, iv);
//! let plaintext = decrypting_key.decrypt(&mut plaintext).unwrap();
//! ```
//!

#![allow(clippy::module_name_repetitions)]

pub(crate) mod aes;
pub(crate) mod block;
pub(crate) mod chacha;

use crate::cipher::aes::{encrypt_block_aes, Aes128Key, Aes256Key};
use crate::cipher::block::Block;
use crate::cipher::chacha::ChaCha20Key;
use crate::error::Unspecified;
use crate::iv::{self, FixedLength};
use aws_lc::{
    AES_cbc_encrypt, AES_ctr128_encrypt, AES_set_decrypt_key, AES_set_encrypt_key, AES_DECRYPT,
    AES_ENCRYPT, AES_KEY,
};
use std::mem::{size_of, transmute, MaybeUninit};
use std::os::raw::c_uint;
use std::ptr;
use zeroize::Zeroize;

pub(crate) enum SymmetricCipherKey {
    Aes128 {
        raw_key: Aes128Key,
        enc_key: AES_KEY,
        dec_key: AES_KEY,
    },
    Aes256 {
        raw_key: Aes256Key,
        enc_key: AES_KEY,
        dec_key: AES_KEY,
    },
    ChaCha20 {
        raw_key: ChaCha20Key,
    },
}

unsafe impl Send for SymmetricCipherKey {}
// The AES_KEY value is only used as a `*const AES_KEY` in calls to `AES_encrypt`.
unsafe impl Sync for SymmetricCipherKey {}

impl Drop for SymmetricCipherKey {
    fn drop(&mut self) {
        // Aes128Key, Aes256Key and ChaCha20Key implement Drop separately.
        match self {
            SymmetricCipherKey::Aes128 {
                enc_key, dec_key, ..
            }
            | SymmetricCipherKey::Aes256 {
                enc_key, dec_key, ..
            } => unsafe {
                #[allow(clippy::transmute_ptr_to_ptr)]
                let enc_bytes: &mut [u8; size_of::<AES_KEY>()] = transmute(enc_key);
                enc_bytes.zeroize();
                #[allow(clippy::transmute_ptr_to_ptr)]
                let dec_bytes: &mut [u8; size_of::<AES_KEY>()] = transmute(dec_key);
                dec_bytes.zeroize();
            },
            SymmetricCipherKey::ChaCha20 { .. } => {}
        }
    }
}

/// The cipher block padding strategy.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum PaddingStrategy {
    /// PKCS#7 Padding.
    PKCS7,
}

impl PaddingStrategy {
    fn add_padding<InOut>(self, block_len: usize, in_out: &mut InOut) -> Result<(), Unspecified>
    where
        InOut: AsMut<[u8]> + for<'in_out> Extend<&'in_out u8>,
    {
        match self {
            PaddingStrategy::PKCS7 => {
                let in_out_len = in_out.as_mut().len();
                // This implements PKCS#7 padding scheme, used by aws-lc if we were using EVP_CIPHER API's
                let remainder = in_out_len % block_len;
                if remainder == 0 {
                    let block_size: u8 = block_len.try_into().map_err(|_| Unspecified)?;
                    in_out.extend(vec![block_size; block_len].iter());
                } else {
                    let padding_size = block_len - remainder;
                    let v: u8 = padding_size.try_into().map_err(|_| Unspecified)?;
                    // Heap allocation :(
                    in_out.extend(vec![v; padding_size].iter());
                }
            }
        }
        Ok(())
    }

    fn remove_padding(self, block_len: usize, in_out: &mut [u8]) -> Result<&mut [u8], Unspecified> {
        match self {
            PaddingStrategy::PKCS7 => {
                let block_size: u8 = block_len.try_into().map_err(|_| Unspecified)?;

                if in_out.is_empty() || in_out.len() < block_len {
                    return Err(Unspecified);
                }

                let padding: u8 = in_out[in_out.len() - 1];
                if padding == 0 || padding > block_size {
                    return Err(Unspecified);
                }

                for item in in_out.iter().skip(in_out.len() - padding as usize) {
                    if *item != padding {
                        return Err(Unspecified);
                    }
                }

                let final_len = in_out.len() - padding as usize;
                Ok(&mut in_out[0..final_len])
            }
        }
    }
}

/// The number of bytes in an AES 128-bit key
pub const AES_128_KEY_LEN: usize = 16;

/// The number of bytes in an AES 256-bit key
pub const AES_256_KEY_LEN: usize = 32;

/// The number of bytes for an AES initalization vector (IV)
pub const AES_IV_LEN: usize = 16;
const AES_BLOCK_LEN: usize = 16;

/// The cipher operating mode.
#[non_exhaustive]
#[derive(Clone, Copy)]
pub enum OperatingMode {
    /// Cipher block chaining (CBC) mode.
    CBC,

    /// Counter (CTR) mode.
    CTR,
}

/// The contextual data used to encrypted/decrypt data.
#[non_exhaustive]
pub enum DecryptionContext {
    /// A 128-bit Initalization Vector.
    Iv128(iv::FixedLength<16>),

    /// No input to the cipher mode.
    None,
}

#[non_exhaustive]
/// Cipher algorithm identifier.
pub enum AlgorithmId {
    /// AES 128-bit
    Aes128,

    /// AES 256-bit
    Aes256,
}

/// A cipher algorithm.
pub struct Algorithm {
    id: AlgorithmId,
    key_len: usize,
    block_len: usize,
}

/// AES 128-bit cipher
pub const AES_128: Algorithm = Algorithm {
    id: AlgorithmId::Aes128,
    key_len: AES_128_KEY_LEN,
    block_len: AES_BLOCK_LEN,
};

/// AES 256-bit cipher
pub const AES_256: Algorithm = Algorithm {
    id: AlgorithmId::Aes256,
    key_len: AES_256_KEY_LEN,
    block_len: AES_BLOCK_LEN,
};

impl Algorithm {
    fn id(&self) -> &AlgorithmId {
        &self.id
    }

    #[allow(unused)]
    const fn key_len(&self) -> usize {
        self.key_len
    }

    const fn block_len(&self) -> usize {
        self.block_len
    }

    fn new_decrypting_context(
        &self,
        mode: OperatingMode,
    ) -> Result<DecryptionContext, Unspecified> {
        match self.id {
            AlgorithmId::Aes128 | AlgorithmId::Aes256 => match mode {
                OperatingMode::CBC | OperatingMode::CTR => {
                    Ok(DecryptionContext::Iv128(FixedLength::new()?))
                }
            },
        }
    }

    fn is_valid_decryption_context(&self, mode: OperatingMode, input: &DecryptionContext) -> bool {
        match self.id {
            AlgorithmId::Aes128 | AlgorithmId::Aes256 => match mode {
                OperatingMode::CBC | OperatingMode::CTR => match input {
                    DecryptionContext::Iv128(_) => true,
                    _ => false,
                },
            },
        }
    }
}

/// A key bound to a particular cipher algorithm.
pub struct UnboundCipherKey {
    algorithm: &'static Algorithm,
    key: SymmetricCipherKey,
}

impl UnboundCipherKey {
    /// Constructs a [`UnboundCipherKey`].
    ///
    /// # Errors
    ///
    /// * [`Unspecified`] if `key_bytes.len()` does not match the
    /// length required by `algorithm`.
    ///
    pub fn new(algorithm: &'static Algorithm, key_bytes: &[u8]) -> Result<Self, Unspecified> {
        let key = match algorithm.id() {
            AlgorithmId::Aes128 => SymmetricCipherKey::aes128(key_bytes),
            AlgorithmId::Aes256 => SymmetricCipherKey::aes256(key_bytes),
        }?;
        Ok(UnboundCipherKey { algorithm, key })
    }

    #[inline]
    fn algorithm(&self) -> &'static Algorithm {
        self.algorithm
    }
}

/// A cipher encryption key that performs block padding.
pub struct PaddedBlockEncryptingKey {
    key: UnboundCipherKey,
    mode: OperatingMode,
    padding: PaddingStrategy,
    context: DecryptionContext,
}

impl PaddedBlockEncryptingKey {
    /// Constructs a new `PaddedBlockEncryptingKey` cipher with chaining block cipher (CBC) mode.
    /// Plaintext data is padded following the PKCS#7 scheme.
    pub fn cbc_pkcs7(key: UnboundCipherKey) -> Result<PaddedBlockEncryptingKey, Unspecified> {
        PaddedBlockEncryptingKey::new(key, OperatingMode::CBC, PaddingStrategy::PKCS7, None)
    }

    /// Constructs a new `PaddedBlockEncryptingKey` cipher with chaining block cipher (CBC) mode.
    /// The users provided context will be used for the CBC initalization-vector.
    /// Plaintext data is padded following the PKCS#7 scheme.
    pub fn less_safe_cbc_pkcs7(
        key: UnboundCipherKey,
        context: DecryptionContext,
    ) -> Result<PaddedBlockEncryptingKey, Unspecified> {
        PaddedBlockEncryptingKey::new(
            key,
            OperatingMode::CBC,
            PaddingStrategy::PKCS7,
            Some(context),
        )
    }

    fn new(
        key: UnboundCipherKey,
        mode: OperatingMode,
        padding: PaddingStrategy,
        context: Option<DecryptionContext>,
    ) -> Result<PaddedBlockEncryptingKey, Unspecified> {
        let mode_input = match context {
            Some(mi) => {
                if !key.algorithm().is_valid_decryption_context(mode, &mi) {
                    return Err(Unspecified);
                }
                mi
            }
            None => key.algorithm.new_decrypting_context(mode)?,
        };

        Ok(PaddedBlockEncryptingKey {
            key,
            mode,
            padding,
            context: mode_input,
        })
    }

    /// Returns the cipher algorithm.
    pub fn algorithm(&self) -> &Algorithm {
        self.key.algorithm()
    }

    /// Returns the cipher operating mode.
    pub fn mode(&self) -> OperatingMode {
        self.mode
    }

    /// Returns the cipher padding strategy.
    pub fn padding(&self) -> PaddingStrategy {
        self.padding
    }

    /// Pads and encrypts data provided in `in_out` in-place.
    /// Returns a references to the encryted data.
    ///
    /// # Errors
    /// * [`Unspecified`]: Returned if encryption fails.
    ///
    pub fn encrypt<InOut>(self, in_out: &mut InOut) -> Result<DecryptionContext, Unspecified>
    where
        InOut: AsMut<[u8]> + for<'a> Extend<&'a u8>,
    {
        self.padding
            .add_padding(self.algorithm().block_len(), in_out)?;
        TryInto::<EncryptingKey>::try_into(self)?.encrypt(in_out.as_mut())
    }
}

impl TryFrom<PaddedBlockEncryptingKey> for EncryptingKey {
    type Error = Unspecified;

    fn try_from(value: PaddedBlockEncryptingKey) -> Result<Self, Self::Error> {
        EncryptingKey::new(value.key, value.mode, Some(value.context))
    }
}

/// A cipher decryption key that performs block padding.
pub struct PaddedBlockDecryptingKey {
    key: UnboundCipherKey,
    mode: OperatingMode,
    padding: PaddingStrategy,
    mode_input: DecryptionContext,
}

impl PaddedBlockDecryptingKey {
    /// Constructs a new `PaddedBlockDecryptingKey` cipher with chaining block cipher (CBC) mode.
    /// Decrypted data is unpadded following the PKCS#7 scheme.
    pub fn cbc_pkcs7(
        key: UnboundCipherKey,
        mode_input: DecryptionContext,
    ) -> Result<PaddedBlockDecryptingKey, Unspecified> {
        PaddedBlockDecryptingKey::new(key, OperatingMode::CBC, PaddingStrategy::PKCS7, mode_input)
    }

    fn new(
        key: UnboundCipherKey,
        mode: OperatingMode,
        padding: PaddingStrategy,
        mode_input: DecryptionContext,
    ) -> Result<PaddedBlockDecryptingKey, Unspecified> {
        if !key
            .algorithm()
            .is_valid_decryption_context(mode, &mode_input)
        {
            return Err(Unspecified);
        }

        Ok(PaddedBlockDecryptingKey {
            key,
            mode,
            padding,
            mode_input,
        })
    }

    /// Returns the cipher algorithm.
    pub fn algorithm(&self) -> &Algorithm {
        self.key.algorithm()
    }

    /// Returns the cipher operating mode.
    pub fn mode(&self) -> OperatingMode {
        self.mode
    }

    /// Returns the cipher padding strategy.
    pub fn padding(&self) -> PaddingStrategy {
        self.padding
    }

    /// Decrypts and unpads data provided in `in_out` in-place.
    /// Returns a references to the decrypted data.
    ///
    /// # Errors
    /// * [`Unspecified`]: Returned if decryption fails.
    ///
    pub fn decrypt<'a>(self, in_out: &'a mut [u8]) -> Result<&'a mut [u8], Unspecified> {
        let block_len = self.algorithm().block_len();
        let padding = self.padding;
        let mut in_out = TryInto::<DecryptingKey>::try_into(self)?.decrypt(in_out)?;
        in_out = padding.remove_padding(block_len, in_out)?;
        Ok(in_out)
    }
}

/// A cipher encryption key that does not perform block padding.
pub struct EncryptingKey {
    key: UnboundCipherKey,
    mode: OperatingMode,
    context: DecryptionContext,
}

impl EncryptingKey {
    /// Constructs an `EncryptingKey` operating in counter (CTR) mode using the provided key.
    pub fn ctr(key: UnboundCipherKey) -> Result<EncryptingKey, Unspecified> {
        EncryptingKey::new(key, OperatingMode::CTR, None)
    }

    /// Constructs an `EncryptingKey` operating in counter (CTR) mode using the provided key.
    /// The users provided context will be used for the CTR mode initalization-vector.
    pub fn less_safe_ctr(
        key: UnboundCipherKey,
        context: DecryptionContext,
    ) -> Result<EncryptingKey, Unspecified> {
        EncryptingKey::new(key, OperatingMode::CTR, Some(context))
    }

    fn new(
        key: UnboundCipherKey,
        mode: OperatingMode,
        context: Option<DecryptionContext>,
    ) -> Result<EncryptingKey, Unspecified> {
        let context = match context {
            Some(mi) => {
                if !key.algorithm().is_valid_decryption_context(mode, &mi) {
                    return Err(Unspecified);
                }
                mi
            }
            None => key.algorithm.new_decrypting_context(mode)?,
        };

        Ok(EncryptingKey { key, mode, context })
    }

    /// Returns the cipher algorithm.
    pub fn algorithm(&self) -> &Algorithm {
        self.key.algorithm()
    }

    /// Returns the cipher operating mode.
    pub fn mode(&self) -> OperatingMode {
        self.mode
    }

    /// Encrypts the data provided in `in_out` in-place.
    /// Returns a references to the decrypted data.
    ///
    /// # Errors
    /// * [`Unspecified`]: Returned if cipher mode requires input to be a multiple of the block length,
    /// and `in_out.len()` is not. Otherwise returned if encryption fails.
    ///
    pub fn encrypt(self, in_out: &mut [u8]) -> Result<DecryptionContext, Unspecified> {
        let block_len = self.algorithm().block_len();

        match self.mode {
            OperatingMode::CTR => {}
            _ => {
                if (in_out.len() % block_len) != 0 {
                    return Err(Unspecified);
                }
            }
        }

        match self.mode {
            OperatingMode::CBC => match self.key.algorithm().id() {
                AlgorithmId::Aes128 | AlgorithmId::Aes256 => {
                    encrypt_aes_cbc_mode(self.key, self.context, in_out)
                }
            },
            OperatingMode::CTR => match self.key.algorithm().id() {
                AlgorithmId::Aes128 | AlgorithmId::Aes256 => {
                    encrypt_aes_ctr_mode(self.key, self.context, in_out)
                }
            },
        }
    }
}

/// A cipher decryption key that does not perform block padding.
pub struct DecryptingKey {
    key: UnboundCipherKey,
    mode: OperatingMode,
    mode_input: DecryptionContext,
}

impl DecryptingKey {
    /// Constructs a cipher decrypting key operating in counter (CTR) mode using the provided key and context.
    pub fn ctr(
        key: UnboundCipherKey,
        context: DecryptionContext,
    ) -> Result<DecryptingKey, Unspecified> {
        DecryptingKey::new(key, OperatingMode::CTR, context)
    }

    fn new(
        key: UnboundCipherKey,
        mode: OperatingMode,
        mode_input: DecryptionContext,
    ) -> Result<DecryptingKey, Unspecified> {
        if !key
            .algorithm()
            .is_valid_decryption_context(mode, &mode_input)
        {
            return Err(Unspecified);
        }

        Ok(DecryptingKey {
            key,
            mode,
            mode_input,
        })
    }

    /// Returns the cipher algorithm.
    pub fn algorithm(&self) -> &Algorithm {
        self.key.algorithm()
    }

    /// Returns the cipher operating mode.
    pub fn mode(&self) -> OperatingMode {
        self.mode
    }

    /// Decrypts the data provided in `in_out` in-place.
    /// Returns a references to the decrypted data.
    ///
    /// # Errors
    /// * [`Unspecified`]: Returned if cipher mode requires input to be a multiple of the block length,
    /// and `in_out.len()` is not. Otherwise returned if decryption fails.
    ///
    pub fn decrypt<'a>(self, in_out: &'a mut [u8]) -> Result<&'a mut [u8], Unspecified> {
        let block_len = self.algorithm().block_len();

        match self.mode {
            OperatingMode::CTR => {}
            _ => {
                if (in_out.len() % block_len) != 0 {
                    return Err(Unspecified);
                }
            }
        }

        match self.mode {
            OperatingMode::CBC => match self.key.algorithm().id() {
                AlgorithmId::Aes128 | AlgorithmId::Aes256 => {
                    decrypt_aes_cbc_mode(self.key, self.mode_input, in_out).map(|_| in_out)
                }
            },
            OperatingMode::CTR => match self.key.algorithm().id() {
                AlgorithmId::Aes128 | AlgorithmId::Aes256 => {
                    decrypt_aes_ctr_mode(self.key, self.mode_input, in_out).map(|_| in_out)
                }
            },
        }
    }
}

impl TryFrom<PaddedBlockDecryptingKey> for DecryptingKey {
    type Error = Unspecified;

    fn try_from(value: PaddedBlockDecryptingKey) -> Result<Self, Self::Error> {
        DecryptingKey::new(value.key, value.mode, value.mode_input)
    }
}

fn encrypt_aes_ctr_mode(
    key: UnboundCipherKey,
    context: DecryptionContext,
    in_out: &mut [u8],
) -> Result<DecryptionContext, Unspecified> {
    let key = match &key.key {
        SymmetricCipherKey::Aes128 { enc_key, .. } => enc_key,
        SymmetricCipherKey::Aes256 { enc_key, .. } => enc_key,
        _ => return Err(Unspecified),
    };

    let mut iv = {
        let mut iv = [0u8; AES_IV_LEN];
        iv.copy_from_slice(match &context {
            DecryptionContext::Iv128(iv) => iv.as_ref(),
            _ => return Err(Unspecified),
        });
        iv
    };

    let mut buffer = [0u8; AES_BLOCK_LEN];

    aes_ctr128_encrypt(key, &mut iv, &mut buffer, in_out);

    Ok(context)
}

fn decrypt_aes_ctr_mode(
    key: UnboundCipherKey,
    context: DecryptionContext,
    in_out: &mut [u8],
) -> Result<DecryptionContext, Unspecified> {
    // it's the same in CTR, just providing a nice named wrapper to match
    encrypt_aes_ctr_mode(key, context, in_out)
}

fn encrypt_aes_cbc_mode(
    key: UnboundCipherKey,
    context: DecryptionContext,
    in_out: &mut [u8],
) -> Result<DecryptionContext, Unspecified> {
    let key = match &key.key {
        SymmetricCipherKey::Aes128 { enc_key, .. } => enc_key,
        SymmetricCipherKey::Aes256 { enc_key, .. } => enc_key,
        _ => return Err(Unspecified),
    };

    let mut iv = {
        let mut iv = [0u8; AES_IV_LEN];
        iv.copy_from_slice(match &context {
            DecryptionContext::Iv128(iv) => iv.as_ref(),
            _ => return Err(Unspecified),
        });
        iv
    };

    aes_cbc_encrypt(key, &mut iv, in_out);

    Ok(context)
}

fn decrypt_aes_cbc_mode(
    key: UnboundCipherKey,
    context: DecryptionContext,
    in_out: &mut [u8],
) -> Result<DecryptionContext, Unspecified> {
    let key = match &key.key {
        SymmetricCipherKey::Aes128 { dec_key, .. } => dec_key,
        SymmetricCipherKey::Aes256 { dec_key, .. } => dec_key,
        _ => return Err(Unspecified),
    };

    let mut iv = {
        let mut iv = [0u8; AES_IV_LEN];
        iv.copy_from_slice(match &context {
            DecryptionContext::Iv128(iv) => iv.as_ref(),
            _ => return Err(Unspecified),
        });
        iv
    };

    aes_cbc_decrypt(key, &mut iv, in_out);

    Ok(context)
}

fn aes_ctr128_encrypt(key: &AES_KEY, iv: &mut [u8], block_buffer: &mut [u8], in_out: &mut [u8]) {
    let mut num = MaybeUninit::<u32>::new(0);

    unsafe {
        AES_ctr128_encrypt(
            in_out.as_ptr(),
            in_out.as_mut_ptr(),
            in_out.len(),
            key,
            iv.as_mut_ptr(),
            block_buffer.as_mut_ptr(),
            num.as_mut_ptr(),
        );
    };

    Zeroize::zeroize(block_buffer);
}

fn aes_cbc_encrypt(key: &AES_KEY, iv: &mut [u8], in_out: &mut [u8]) {
    unsafe {
        AES_cbc_encrypt(
            in_out.as_ptr(),
            in_out.as_mut_ptr(),
            in_out.len(),
            key,
            iv.as_mut_ptr(),
            AES_ENCRYPT,
        );
    }
}

fn aes_cbc_decrypt(key: &AES_KEY, iv: &mut [u8], in_out: &mut [u8]) {
    unsafe {
        AES_cbc_encrypt(
            in_out.as_ptr(),
            in_out.as_mut_ptr(),
            in_out.len(),
            key,
            iv.as_mut_ptr(),
            AES_DECRYPT,
        );
    }
}

impl SymmetricCipherKey {
    pub(crate) fn aes128(key_bytes: &[u8]) -> Result<Self, Unspecified> {
        if key_bytes.len() != AES_128_KEY_LEN {
            return Err(Unspecified);
        }

        unsafe {
            let mut enc_key = MaybeUninit::<AES_KEY>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            if 0 != AES_set_encrypt_key(
                key_bytes.as_ptr(),
                (key_bytes.len() * 8) as c_uint,
                enc_key.as_mut_ptr(),
            ) {
                return Err(Unspecified);
            }
            let enc_key = enc_key.assume_init();

            let mut dec_key = MaybeUninit::<AES_KEY>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            if 0 != AES_set_decrypt_key(
                key_bytes.as_ptr(),
                (key_bytes.len() * 8) as c_uint,
                dec_key.as_mut_ptr(),
            ) {
                return Err(Unspecified);
            }
            let dec_key = dec_key.assume_init();

            let mut kb = MaybeUninit::<[u8; 16]>::uninit();
            ptr::copy_nonoverlapping(key_bytes.as_ptr(), kb.as_mut_ptr().cast(), 16);
            Ok(SymmetricCipherKey::Aes128 {
                raw_key: Aes128Key(kb.assume_init()),
                enc_key,
                dec_key,
            })
        }
    }

    pub(crate) fn aes256(key_bytes: &[u8]) -> Result<Self, Unspecified> {
        if key_bytes.len() != 32 {
            return Err(Unspecified);
        }
        unsafe {
            let mut enc_key = MaybeUninit::<AES_KEY>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            if 0 != AES_set_encrypt_key(
                key_bytes.as_ptr(),
                (key_bytes.len() * 8) as c_uint,
                enc_key.as_mut_ptr(),
            ) {
                return Err(Unspecified);
            }
            let enc_key = enc_key.assume_init();

            let mut dec_key = MaybeUninit::<AES_KEY>::uninit();
            #[allow(clippy::cast_possible_truncation)]
            if 0 != AES_set_decrypt_key(
                key_bytes.as_ptr(),
                (key_bytes.len() * 8) as c_uint,
                dec_key.as_mut_ptr(),
            ) {
                return Err(Unspecified);
            }
            let dec_key = dec_key.assume_init();

            let mut kb = MaybeUninit::<[u8; 32]>::uninit();
            ptr::copy_nonoverlapping(key_bytes.as_ptr(), kb.as_mut_ptr().cast(), 32);
            Ok(SymmetricCipherKey::Aes256 {
                raw_key: Aes256Key(kb.assume_init()),
                enc_key,
                dec_key,
            })
        }
    }

    pub(crate) fn chacha20(key_bytes: &[u8]) -> Result<Self, Unspecified> {
        if key_bytes.len() != 32 {
            return Err(Unspecified);
        }
        let mut kb = MaybeUninit::<[u8; 32]>::uninit();
        unsafe {
            ptr::copy_nonoverlapping(key_bytes.as_ptr(), kb.as_mut_ptr().cast(), 32);
            Ok(SymmetricCipherKey::ChaCha20 {
                raw_key: ChaCha20Key(kb.assume_init()),
            })
        }
    }

    #[inline]
    pub(super) fn key_bytes(&self) -> &[u8] {
        match self {
            SymmetricCipherKey::Aes128 { raw_key, .. } => &raw_key.0,
            SymmetricCipherKey::Aes256 { raw_key, .. } => &raw_key.0,
            SymmetricCipherKey::ChaCha20 { raw_key, .. } => &raw_key.0,
        }
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn encrypt_block(&self, block: Block) -> Block {
        match self {
            SymmetricCipherKey::Aes128 { enc_key, .. }
            | SymmetricCipherKey::Aes256 { enc_key, .. } => encrypt_block_aes(enc_key, block),
            SymmetricCipherKey::ChaCha20 { .. } => panic!("Unsupported algorithm!"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher::block::BLOCK_LEN;
    use crate::test::from_hex;

    #[test]
    fn test_encrypt_block_aes_128() {
        let key = from_hex("000102030405060708090a0b0c0d0e0f").unwrap();
        let input = from_hex("00112233445566778899aabbccddeeff").unwrap();
        let expected_result = from_hex("69c4e0d86a7b0430d8cdb78070b4c55a").unwrap();
        let input_block: [u8; BLOCK_LEN] = <[u8; BLOCK_LEN]>::try_from(input).unwrap();

        let aes128 = SymmetricCipherKey::aes128(key.as_slice()).unwrap();
        let result = aes128.encrypt_block(Block::from(&input_block));

        assert_eq!(expected_result.as_slice(), result.as_ref());
    }

    #[test]
    fn test_encrypt_block_aes_256() {
        let key =
            from_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f").unwrap();
        let input = from_hex("00112233445566778899aabbccddeeff").unwrap();
        let expected_result = from_hex("8ea2b7ca516745bfeafc49904b496089").unwrap();
        let input_block: [u8; BLOCK_LEN] = <[u8; BLOCK_LEN]>::try_from(input).unwrap();

        let aes128 = SymmetricCipherKey::aes256(key.as_slice()).unwrap();
        let result = aes128.encrypt_block(Block::from(&input_block));

        assert_eq!(expected_result.as_slice(), result.as_ref());
    }

    fn helper_test_cipher_n_bytes(
        key: &[u8],
        alg: &'static Algorithm,
        mode: OperatingMode,
        n: usize,
    ) {
        let mut input: Vec<u8> = Vec::with_capacity(n);
        for i in 0..n {
            let byte: u8 = i.try_into().unwrap();
            input.push(byte);
        }

        let cipher_key = UnboundCipherKey::new(alg, key).unwrap();
        let encrypting_key = EncryptingKey::new(cipher_key, mode, None).unwrap();

        let mut in_out = input.clone();
        let decrypt_iv = encrypting_key.encrypt(&mut in_out).unwrap();

        if n > 5 {
            // There's no more than a 1 in 2^48 chance that this will fail randomly
            assert_ne!(input.as_slice(), in_out);
        }

        let cipher_key2 = UnboundCipherKey::new(alg, key).unwrap();
        let decrypting_key = DecryptingKey::new(cipher_key2, mode, decrypt_iv).unwrap();

        let plaintext = decrypting_key.decrypt(&mut in_out).unwrap();
        assert_eq!(input.as_slice(), plaintext);
    }

    fn helper_test_padded_cipher_n_bytes(
        key: &[u8],
        alg: &'static Algorithm,
        mode: OperatingMode,
        padding: PaddingStrategy,
        n: usize,
    ) {
        let mut input: Vec<u8> = Vec::with_capacity(n);
        for i in 0..n {
            let byte: u8 = i.try_into().unwrap();
            input.push(byte);
        }

        let cipher_key = UnboundCipherKey::new(alg, key).unwrap();
        let encrypting_key =
            PaddedBlockEncryptingKey::new(cipher_key, mode, padding, None).unwrap();

        let mut in_out = input.clone();
        let decrypt_iv = encrypting_key.encrypt(&mut in_out).unwrap();

        if n > 5 {
            // There's no more than a 1 in 2^48 chance that this will fail randomly
            assert_ne!(input.as_slice(), in_out);
        }

        let cipher_key2 = UnboundCipherKey::new(alg, key).unwrap();
        let decrypting_key =
            PaddedBlockDecryptingKey::new(cipher_key2, mode, padding, decrypt_iv).unwrap();

        let plaintext = decrypting_key.decrypt(&mut in_out).unwrap();
        assert_eq!(input.as_slice(), plaintext);
    }

    #[test]
    fn test_aes_128_cbc() {
        let key = from_hex("000102030405060708090a0b0c0d0e0f").unwrap();
        for i in 0..=50 {
            helper_test_padded_cipher_n_bytes(
                key.as_slice(),
                &AES_128,
                OperatingMode::CBC,
                PaddingStrategy::PKCS7,
                i,
            );
        }
    }

    #[test]
    fn test_aes_256_cbc() {
        let key =
            from_hex("000102030405060708090a0b0c0d0e0f000102030405060708090a0b0c0d0e0f").unwrap();
        for i in 0..=50 {
            helper_test_padded_cipher_n_bytes(
                key.as_slice(),
                &AES_256,
                OperatingMode::CBC,
                PaddingStrategy::PKCS7,
                i,
            );
        }
    }

    #[test]
    fn test_aes_128_ctr() {
        let key = from_hex("000102030405060708090a0b0c0d0e0f").unwrap();
        for i in 0..=50 {
            helper_test_cipher_n_bytes(key.as_slice(), &AES_128, OperatingMode::CTR, i);
        }
    }

    #[test]
    fn test_aes_256_ctr() {
        let key =
            from_hex("000102030405060708090a0b0c0d0e0f000102030405060708090a0b0c0d0e0f").unwrap();
        for i in 0..=50 {
            helper_test_cipher_n_bytes(key.as_slice(), &AES_256, OperatingMode::CTR, i);
        }
    }

    macro_rules! padded_cipher_kat {
        ($name:ident, $alg:expr, $mode:expr, $padding:expr, $key:literal, $iv: literal, $plaintext:literal, $ciphertext:literal) => {
            #[test]
            fn $name() {
                let key = from_hex($key).unwrap();
                let input = from_hex($plaintext).unwrap();
                let expected_ciphertext = from_hex($ciphertext).unwrap();
                let mut iv = from_hex($iv).unwrap();
                let iv = {
                    let slice = iv.as_mut_slice();
                    let mut iv = [0u8; $iv.len() / 2];
                    {
                        let x = iv.as_mut_slice();
                        x.copy_from_slice(slice);
                    }
                    iv
                };

                let dc = DecryptionContext::Iv128(FixedLength::from(iv));

                let alg = $alg;

                let unbound_key = UnboundCipherKey::new(alg, &key).unwrap();

                let encrypting_key =
                    PaddedBlockEncryptingKey::new(unbound_key, $mode, $padding, Some(dc)).unwrap();

                let mut in_out = input.clone();

                let context = encrypting_key.encrypt(&mut in_out).unwrap();

                assert_eq!(expected_ciphertext, in_out);

                let unbound_key2 = UnboundCipherKey::new(alg, &key).unwrap();
                let decrypting_key =
                    PaddedBlockDecryptingKey::new(unbound_key2, $mode, $padding, context).unwrap();

                let plaintext = decrypting_key.decrypt(&mut in_out).unwrap();
                assert_eq!(input.as_slice(), plaintext);
            }
        };
    }

    macro_rules! cipher_kat {
        ($name:ident, $alg:expr, $mode:expr, $key:literal, $iv: literal, $plaintext:literal, $ciphertext:literal) => {
            #[test]
            fn $name() {
                let key = from_hex($key).unwrap();
                let input = from_hex($plaintext).unwrap();
                let expected_ciphertext = from_hex($ciphertext).unwrap();
                let mut iv = from_hex($iv).unwrap();
                let iv = {
                    let slice = iv.as_mut_slice();
                    let mut iv = [0u8; $iv.len() / 2];
                    {
                        let x = iv.as_mut_slice();
                        x.copy_from_slice(slice);
                    }
                    iv
                };

                let dc = DecryptionContext::Iv128(FixedLength::from(iv));

                let alg = $alg;

                let unbound_key = UnboundCipherKey::new(alg, &key).unwrap();

                let encrypting_key = EncryptingKey::new(unbound_key, $mode, Some(dc)).unwrap();

                let mut in_out = input.clone();

                let context = encrypting_key.encrypt(&mut in_out).unwrap();

                assert_eq!(expected_ciphertext, in_out);

                let unbound_key2 = UnboundCipherKey::new(alg, &key).unwrap();
                let decrypting_key = DecryptingKey::new(unbound_key2, $mode, context).unwrap();

                let plaintext = decrypting_key.decrypt(&mut in_out).unwrap();
                assert_eq!(input.as_slice(), plaintext);
            }
        };
    }

    padded_cipher_kat!(
        test_iv_aes_128_cbc_16_bytes,
        &AES_128,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "000102030405060708090a0b0c0d0e0f",
        "00000000000000000000000000000000",
        "00112233445566778899aabbccddeeff",
        "69c4e0d86a7b0430d8cdb78070b4c55a9e978e6d16b086570ef794ef97984232"
    );

    padded_cipher_kat!(
        test_iv_aes_256_cbc_15_bytes,
        &AES_256,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "00000000000000000000000000000000",
        "00112233445566778899aabbccddee",
        "2ddfb635a651a43f582997966840ca0c"
    );

    cipher_kat!(
        test_iv_aes_128_ctr_16_bytes,
        &AES_128,
        OperatingMode::CTR,
        "000102030405060708090a0b0c0d0e0f",
        "00000000000000000000000000000000",
        "00112233445566778899aabbccddeeff",
        "c6b01904c3da3df5e7d62bd96d153686"
    );

    cipher_kat!(
        test_iv_aes_256_ctr_15_bytes,
        &AES_256,
        OperatingMode::CTR,
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "00000000000000000000000000000000",
        "00112233445566778899aabbccddee",
        "f28122856e1cf9a7216a30d111f399"
    );

    cipher_kat!(
        test_openssl_aes_128_ctr_15_bytes,
        &AES_128,
        OperatingMode::CTR,
        "244828580821c1652582c76e34d299f5",
        "093145d5af233f46072a5eb5adc11aa1",
        "3ee38cec171e6cf466bf0df98aa0e1",
        "bd7d928f60e3422d96b3f8cd614eb2"
    );

    cipher_kat!(
        test_openssl_aes_256_ctr_15_bytes,
        &AES_256,
        OperatingMode::CTR,
        "0857db8240ea459bdf660b4cced66d1f2d3734ff2de7b81e92740e65e7cc6a1d",
        "f028ecb053f801102d11fccc9d303a27",
        "eca7285d19f3c20e295378460e8729",
        "b5098e5e788de6ac2f2098eb2fc6f8"
    );

    padded_cipher_kat!(
        test_openssl_aes_128_cbc_15_bytes,
        &AES_128,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "053304bb3899e1d99db9d29343ea782d",
        "b5313560244a4822c46c2a0c9d0cf7fd",
        "a3e4c990356c01f320043c3d8d6f43",
        "ad96993f248bd6a29760ec7ccda95ee1"
    );

    padded_cipher_kat!(
        test_openssl_aes_128_cbc_16_bytes,
        &AES_128,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "95af71f1c63e4a1d0b0b1a27fb978283",
        "89e40797dca70197ff87d3dbb0ef2802",
        "aece7b5e3c3df1ffc9802d2dfe296dc7",
        "301b5dab49fb11e919d0d39970d06739301919743304f23f3cbc67d28564b25b"
    );

    padded_cipher_kat!(
        test_openssl_aes_256_cbc_15_bytes,
        &AES_256,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "d369e03e9752784917cc7bac1db7399598d9555e691861d9dd7b3292a693ef57",
        "1399bb66b2f6ad99a7f064140eaaa885",
        "7385f5784b85bf0a97768ddd896d6d",
        "4351082bac9b4593ae8848cc9dfb5a01"
    );

    padded_cipher_kat!(
        test_openssl_aes_256_cbc_16_bytes,
        &AES_256,
        OperatingMode::CBC,
        PaddingStrategy::PKCS7,
        "d4a8206dcae01242f9db79a4ecfe277d0f7bb8ccbafd8f9809adb39f35aa9b41",
        "24f6076548fb9d93c8f7ed9f6e661ef9",
        "a39c1fdf77ea3e1f18178c0ec237c70a",
        "f1af484830a149ee0387b854d65fe87ca0e62efc1c8e6909d4b9ab8666470453"
    );
}
