//! Read-path integrity checks shared by object, multipart, and replica reads.
//!
//! Two different checks live here and they establish different things, so the
//! distinction is drawn explicitly rather than left to the reader.
//!
//! [`verify_physical_length`] runs **before any byte is released**. It compares
//! what the filesystem says the payload occupies against what committed
//! metadata says it should, which is what catches a truncated or extended file
//! — the ordinary shape of storage corruption — while the caller can still be
//! handed a clean error instead of a short body.
//!
//! [`verifying_stream`] runs **while bytes are streaming**. It recomputes the
//! payload digest and fails the stream before its final chunk is delivered when
//! the digest disagrees with the one recorded at commit. That is detection, not
//! prevention: bytes have already left by the time the mismatch is known, so
//! the guarantee is that a corrupt read *fails* rather than that no corrupt byte
//! is ever observed. Verifying before release would mean reading every payload
//! twice, which for object storage is not a trade worth making.
//!
//! A ranged read gets the length check but no digest check, because a digest
//! over part of a payload cannot be compared with a digest over all of it.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
use record_store_core::Checksum;
use sha2::{Digest, Sha256};

use crate::{DownloadStream, StorageError};

/// Refuses a payload whose physical size cannot hold the committed logical size.
///
/// `physical` is what the file occupies on disk and `expected` is what the
/// payload format says a payload of `logical` bytes must occupy. They are
/// computed by the caller because only it knows the format's framing overhead.
pub(crate) const fn verify_physical_length(
    physical: u64,
    expected: u64,
) -> Result<(), StorageError> {
    if physical == expected {
        Ok(())
    } else {
        Err(StorageError::IntegrityMismatch)
    }
}

/// Wraps a download stream so a digest mismatch fails the read instead of the
/// client silently receiving corrupt bytes.
pub(crate) fn verifying_stream(body: DownloadStream, expected: Checksum) -> DownloadStream {
    struct State {
        hasher: Sha256,
        expected: Checksum,
    }
    let state = Arc::new(Mutex::new(Some(State {
        hasher: Sha256::new(),
        expected,
    })));
    let finish = Arc::clone(&state);
    let verified = body
        .map(move |chunk| {
            let chunk = chunk?;
            let mut guard = state.lock().map_err(|_| StorageError::Coordination)?;
            if let Some(state) = guard.as_mut() {
                state.hasher.update(&chunk);
            }
            Ok(chunk)
        })
        .chain(stream::once(async move {
            let taken = finish
                .lock()
                .map_err(|_| StorageError::Coordination)?
                .take();
            match taken {
                Some(state) => {
                    let actual = Checksum::sha256(state.hasher.finalize().into());
                    if actual == state.expected {
                        Ok(Bytes::new())
                    } else {
                        Err(StorageError::IntegrityMismatch)
                    }
                }
                None => Ok(Bytes::new()),
            }
        }))
        .try_filter(|chunk| std::future::ready(!chunk.is_empty()));
    Box::pin(verified)
}
