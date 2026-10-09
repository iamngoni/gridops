//! Checked, bounded transport chunks for owned workload logs and artifacts.
//! Checksums establish byte integrity only. The receiver must authenticate the
//! placement, enforce persisted offsets, and durably store bytes before ack.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

use super::primitives::{ByteOffset, MAX_CHUNK_BYTES};
use crate::fleet::ids::{ArtifactId, BackendId, Generation, LogStreamId, PlacementId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StreamReference {
    Log { stream_id: LogStreamId },
    Artifact { artifact_id: ArtifactId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkOwnership {
    pub placement_id: PlacementId,
    pub backend_id: BackendId,
    pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedByteGap {
    pub from: ByteOffset,
    pub to: ByteOffset,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawChunk", into = "RawChunk")]
pub struct TransferChunk {
    stream: StreamReference,
    owner: ChunkOwnership,
    offset: ByteOffset,
    bytes: Vec<u8>,
    eof: bool,
    gap: Option<DroppedByteGap>,
}

// Raw transport fields are private so callers cannot bypass checked decoding.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChunk {
    stream: StreamReference,
    owner: ChunkOwnership,
    offset: ByteOffset,
    data_base64: String,
    decoded_length: usize,
    sha256: String,
    eof: bool,
    #[serde(default)]
    gap: Option<DroppedByteGap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChunkError {
    #[error("chunk exceeds the decoded byte limit")]
    TooLarge,
    #[error("invalid chunk encoding or checksum")]
    InvalidBytes,
    #[error("chunk offset exceeds the contract range")]
    OffsetOverflow,
    #[error("invalid dropped-byte gap")]
    InvalidGap,
    #[error("empty chunk requires EOF or a dropped-byte gap")]
    Empty,
}

impl TryFrom<RawChunk> for TransferChunk {
    type Error = ChunkError;
    fn try_from(raw: RawChunk) -> Result<Self, Self::Error> {
        if raw.decoded_length > MAX_CHUNK_BYTES
            || raw.data_base64.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4
        {
            return Err(ChunkError::TooLarge);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&raw.data_base64)
            .map_err(|_| ChunkError::InvalidBytes)?;
        if bytes.len() != raw.decoded_length
            || bytes.len() > MAX_CHUNK_BYTES
            || URL_SAFE_NO_PAD.encode(&bytes) != raw.data_base64
            || raw.sha256 != URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes))
        {
            return Err(ChunkError::InvalidBytes);
        }
        let length = u64::try_from(bytes.len()).map_err(|_| ChunkError::TooLarge)?;
        let end = raw
            .offset
            .get()
            .checked_add(length)
            .ok_or(ChunkError::OffsetOverflow)?;
        ByteOffset::new(end).map_err(|_| ChunkError::OffsetOverflow)?;
        if let Some(gap) = &raw.gap {
            if !matches!(raw.stream, StreamReference::Log { .. })
                || gap.from.get() >= gap.to.get()
                || gap.to != raw.offset
            {
                return Err(ChunkError::InvalidGap);
            }
        }
        if bytes.is_empty() && !raw.eof && raw.gap.is_none() {
            return Err(ChunkError::Empty);
        }
        Ok(Self {
            stream: raw.stream,
            owner: raw.owner,
            offset: raw.offset,
            bytes,
            eof: raw.eof,
            gap: raw.gap,
        })
    }
}

impl From<TransferChunk> for RawChunk {
    fn from(chunk: TransferChunk) -> Self {
        Self {
            stream: chunk.stream,
            owner: chunk.owner,
            offset: chunk.offset,
            data_base64: URL_SAFE_NO_PAD.encode(&chunk.bytes),
            decoded_length: chunk.bytes.len(),
            sha256: URL_SAFE_NO_PAD.encode(Sha256::digest(&chunk.bytes)),
            eof: chunk.eof,
            gap: chunk.gap,
        }
    }
}

impl fmt::Debug for TransferChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransferChunk")
            .field("stream", &self.stream)
            .field("owner", &self.owner)
            .field("offset", &self.offset)
            .field("decoded_length", &self.bytes.len())
            .field("eof", &self.eof)
            .field("gap", &self.gap)
            .finish_non_exhaustive()
    }
}

impl TransferChunk {
    /// Build outbound data with the same integrity and offset invariants as
    /// received chunks. The encoded representation is derived from these bytes.
    pub fn from_bytes(
        stream: StreamReference,
        owner: ChunkOwnership,
        offset: ByteOffset,
        bytes: &[u8],
        eof: bool,
        gap: Option<DroppedByteGap>,
    ) -> Result<Self, ChunkError> {
        if bytes.len() > MAX_CHUNK_BYTES {
            return Err(ChunkError::TooLarge);
        }
        Self::try_from(RawChunk {
            stream,
            owner,
            offset,
            data_base64: URL_SAFE_NO_PAD.encode(bytes),
            decoded_length: bytes.len(),
            sha256: URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)),
            eof,
            gap,
        })
    }
    pub fn stream(&self) -> &StreamReference {
        &self.stream
    }
    pub const fn owner(&self) -> &ChunkOwnership {
        &self.owner
    }
    pub const fn offset(&self) -> ByteOffset {
        self.offset
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub const fn is_eof(&self) -> bool {
        self.eof
    }
    pub const fn gap(&self) -> Option<&DroppedByteGap> {
        self.gap.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::super::primitives::{MAX_JSON_BYTES, parse_json};
    use super::*;

    fn raw(bytes: &[u8]) -> Result<RawChunk, Box<dyn std::error::Error>> {
        Ok(RawChunk {
            stream: StreamReference::Log {
                stream_id: "10000000-0000-4000-8000-000000000001".parse()?,
            },
            owner: ChunkOwnership {
                placement_id: "20000000-0000-4000-8000-000000000001".parse()?,
                backend_id: "30000000-0000-4000-8000-000000000001".parse()?,
                generation: Generation::new(1)?,
            },
            offset: ByteOffset::new(0)?,
            data_base64: URL_SAFE_NO_PAD.encode(bytes),
            decoded_length: bytes.len(),
            sha256: URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)),
            eof: false,
            gap: None,
        })
    }

    #[test]
    fn verifies_actual_content_and_redacts_debug() -> Result<(), Box<dyn std::error::Error>> {
        let content = b"sensitive-fixture-output";
        let json = serde_json::to_vec(&raw(content)?)?;
        let chunk = parse_json::<TransferChunk>(&json, MAX_JSON_BYTES)?;
        assert_eq!(chunk.bytes(), content);
        assert_eq!(
            TransferChunk::from_bytes(
                chunk.stream().clone(),
                chunk.owner().clone(),
                chunk.offset(),
                content,
                false,
                None
            )?,
            chunk
        );
        assert_eq!(
            TransferChunk::from_bytes(
                chunk.stream().clone(),
                chunk.owner().clone(),
                chunk.offset(),
                &vec![0; MAX_CHUNK_BYTES + 1],
                false,
                None
            ),
            Err(ChunkError::TooLarge)
        );
        assert!(!format!("{chunk:?}").contains("sensitive"));
        assert!(!format!("{chunk:?}").contains(&URL_SAFE_NO_PAD.encode(content)));
        let serialized = serde_json::to_vec(&chunk)?;
        assert_eq!(
            parse_json::<TransferChunk>(&serialized, MAX_JSON_BYTES)?,
            chunk
        );
        let mut bad = raw(content)?;
        bad.sha256 = URL_SAFE_NO_PAD.encode([0_u8; 32]);
        assert_eq!(TransferChunk::try_from(bad), Err(ChunkError::InvalidBytes));
        let mut bad = raw(content)?;
        bad.decoded_length += 1;
        assert_eq!(TransferChunk::try_from(bad), Err(ChunkError::InvalidBytes));
        let mut bad = raw(content)?;
        bad.data_base64.push('=');
        assert_eq!(TransferChunk::try_from(bad), Err(ChunkError::InvalidBytes));
        Ok(())
    }

    #[test]
    fn enforces_decoded_limits_and_offset_without_wrapping()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(TransferChunk::try_from(raw(&vec![0; MAX_CHUNK_BYTES])?).is_ok());
        assert_eq!(
            TransferChunk::try_from(raw(&vec![0; MAX_CHUNK_BYTES + 1])?),
            Err(ChunkError::TooLarge)
        );
        let mut bad = raw(b"x")?;
        bad.offset = ByteOffset::new(i64::MAX as u64)?;
        assert_eq!(
            TransferChunk::try_from(bad),
            Err(ChunkError::OffsetOverflow)
        );
        assert_eq!(TransferChunk::try_from(raw(b"")?), Err(ChunkError::Empty));
        let mut eof = raw(b"")?;
        eof.eof = true;
        assert!(TransferChunk::try_from(eof)?.is_eof());
        let mut gap = raw(b"")?;
        gap.offset = ByteOffset::new(128)?;
        gap.gap = Some(DroppedByteGap {
            from: ByteOffset::new(0)?,
            to: gap.offset,
        });
        let valid = TransferChunk::try_from(gap)?;
        assert!(valid.gap().is_some());
        let mut artifact: RawChunk = valid.into();
        artifact.stream = StreamReference::Artifact {
            artifact_id: "40000000-0000-4000-8000-000000000001".parse()?,
        };
        assert_eq!(
            TransferChunk::try_from(artifact),
            Err(ChunkError::InvalidGap)
        );
        Ok(())
    }
}
