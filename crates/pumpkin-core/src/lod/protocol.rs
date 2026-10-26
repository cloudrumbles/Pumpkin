//! Encoding of Ironore's four `ironore:` plugin channels.
//!
//! All integers are big-endian and `VarInt` is the protocol's `VarInt`, as in
//! the rest of the protocol. `docs/terrain/lod-generation.md` in Ironore is
//! the specification.

use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_protocol::ClientPacket;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use pumpkin_world::chunk::ChunkData;

use crate::net::java::chunk_data::CChunkData;

pub const REQUEST: &str = "ironore:lod_request";
pub const CHUNK: &str = "ironore:lod_chunk";
pub const ACK: &str = "ironore:lod_ack";
pub const PROGRESS: &str = "ironore:lod_progress";

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("malformed {channel} payload: {source}")]
    Malformed {
        channel: &'static str,
        source: ReadingError,
    },
    #[error("{channel} payload has {count} bytes left over")]
    TrailingBytes { channel: &'static str, count: usize },
}

/// `ironore:lod_request`: the client asks for every generated chunk within
/// `radius` that it does not have. It has a chunk when the chunk's sequence
/// is at most `known` and the chunk lies in the disc `known` covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub radius: i32,
    pub known: i64,
    pub known_centre_x: i32,
    pub known_centre_z: i32,
    pub known_radius: i32,
}

/// `ironore:lod_progress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub total: i32,
    pub generated: i32,
    pub sent: i32,
    pub centre_x: i32,
    pub centre_z: i32,
    /// The highest sequence such that the client has acknowledged every chunk
    /// of the current disc at or below it, or -1.
    pub complete_through: i64,
}

/// Reads an `ironore:lod_request` payload.
///
/// # Errors
///
/// Returns an error if the payload is truncated or has bytes left over.
pub fn read_request(mut data: &[u8]) -> Result<Request, ProtocolError> {
    let malformed = |source| ProtocolError::Malformed {
        channel: REQUEST,
        source,
    };
    let radius = data.get_var_int().map_err(malformed)?.0;
    let known = data.get_i64_be().map_err(malformed)?;
    let known_centre_x = data.get_i32_be().map_err(malformed)?;
    let known_centre_z = data.get_i32_be().map_err(malformed)?;
    let known_radius = data.get_var_int().map_err(malformed)?.0;
    finish(REQUEST, data)?;
    Ok(Request {
        radius,
        known,
        known_centre_x,
        known_centre_z,
        known_radius,
    })
}

/// Reads an `ironore:lod_ack` payload: the number of `lod_chunk` columns the
/// client has applied so far this session.
///
/// # Errors
///
/// Returns an error if the payload is truncated or has bytes left over.
pub fn read_ack(mut data: &[u8]) -> Result<i32, ProtocolError> {
    let applied = data
        .get_var_int()
        .map_err(|source| ProtocolError::Malformed {
            channel: ACK,
            source,
        })?
        .0;
    finish(ACK, data)?;
    Ok(applied)
}

/// Writes an `ironore:lod_chunk` payload: `sequence`, then exactly the body of
/// a vanilla `level_chunk_with_light` packet for `chunk`.
///
/// # Errors
///
/// Returns an error if the chunk cannot be encoded.
pub fn write_chunk(sequence: i64, chunk: &ChunkData) -> Result<Vec<u8>, WritingError> {
    let mut payload = Vec::with_capacity(16 * 1024);
    payload.write_i64_be(sequence)?;
    CChunkData::new(chunk).write_packet_data(&mut payload, &CURRENT_MC_VERSION)?;
    Ok(payload)
}

/// Writes an `ironore:lod_progress` payload.
#[must_use]
pub fn write_progress(progress: &Progress) -> Vec<u8> {
    let mut payload = Vec::with_capacity(31);
    // Writing to a Vec cannot fail.
    let _ = payload.write_var_int(&VarInt(progress.total));
    let _ = payload.write_var_int(&VarInt(progress.generated));
    let _ = payload.write_var_int(&VarInt(progress.sent));
    let _ = payload.write_i32_be(progress.centre_x);
    let _ = payload.write_i32_be(progress.centre_z);
    let _ = payload.write_i64_be(progress.complete_through);
    payload
}

const fn finish(channel: &'static str, rest: &[u8]) -> Result<(), ProtocolError> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(ProtocolError::TrailingBytes {
            channel,
            count: rest.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_payload(radius: i32, known: i64) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(radius)).unwrap();
        payload.write_i64_be(known).unwrap();
        payload.write_i32_be(-3).unwrap();
        payload.write_i32_be(8).unwrap();
        payload.write_var_int(&VarInt(400)).unwrap();
        payload
    }

    #[test]
    fn reads_a_request() {
        let request = read_request(&request_payload(512, 90)).unwrap();
        assert_eq!(
            request,
            Request {
                radius: 512,
                known: 90,
                known_centre_x: -3,
                known_centre_z: 8,
                known_radius: 400,
            }
        );
    }

    #[test]
    fn refuses_a_truncated_request() {
        let payload = request_payload(512, 7);
        assert!(matches!(
            read_request(&payload[..payload.len() - 1]),
            Err(ProtocolError::Malformed { .. })
        ));
    }

    #[test]
    fn refuses_trailing_bytes() {
        let mut payload = request_payload(512, 7);
        payload.push(0);
        assert!(matches!(
            read_request(&payload),
            Err(ProtocolError::TrailingBytes { count: 1, .. })
        ));
        assert!(matches!(
            read_ack(&[5, 0]),
            Err(ProtocolError::TrailingBytes { count: 1, .. })
        ));
    }

    #[test]
    fn reads_an_ack() {
        let mut payload = Vec::new();
        payload.write_var_int(&VarInt(300)).unwrap();
        assert_eq!(read_ack(&payload).unwrap(), 300);
    }

    #[test]
    fn a_chunk_is_its_sequence_then_the_vanilla_body() {
        let chunk = ChunkData::empty(3, -9);
        let payload = write_chunk(42, &chunk).unwrap();
        let mut vanilla = Vec::new();
        CChunkData::new(&chunk)
            .write_packet_data(&mut vanilla, &CURRENT_MC_VERSION)
            .unwrap();
        assert_eq!(&payload[..8], &42i64.to_be_bytes());
        assert_eq!(&payload[8..], &vanilla[..]);
        // The vanilla body opens with the chunk's coordinates.
        assert_eq!(&payload[8..12], &3i32.to_be_bytes());
        assert_eq!(&payload[12..16], &(-9i32).to_be_bytes());
    }

    #[test]
    fn writes_progress() {
        let payload = write_progress(&Progress {
            total: 300,
            generated: 2,
            sent: 1,
            centre_x: -1,
            centre_z: 5,
            complete_through: 77,
        });
        let mut reader = &payload[..];
        assert_eq!(reader.get_var_int().unwrap().0, 300);
        assert_eq!(reader.get_var_int().unwrap().0, 2);
        assert_eq!(reader.get_var_int().unwrap().0, 1);
        assert_eq!(reader.get_i32_be().unwrap(), -1);
        assert_eq!(reader.get_i32_be().unwrap(), 5);
        assert_eq!(reader.get_i64_be().unwrap(), 77);
        assert!(reader.is_empty());
    }
}
