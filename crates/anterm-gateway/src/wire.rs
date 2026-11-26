//! Wire framing helpers for `anterm-gateway` QUIC streams.

use std::io;

use anterm_protocol::gateway::StreamKind;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// Maximum JSON control/header message size on gateway streams (16 MiB).
pub const MAX_GATEWAY_MSG: usize = 16 * 1024 * 1024;

/// Write the single stream-type header byte.
pub async fn write_stream_kind<W: AsyncWrite + Unpin>(
    w: &mut W,
    kind: StreamKind,
) -> io::Result<()> {
    w.write_all(&[kind.as_byte()]).await
}

/// Read and validate the single stream-type header byte.
pub async fn read_stream_kind<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<StreamKind> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).await?;
    StreamKind::from_byte(b[0]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown gateway stream kind 0x{:02x}", b[0]),
        )
    })
}

/// Write a 4-byte little-endian length-prefixed JSON payload.
pub async fn write_json_msg<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    value: &T,
) -> io::Result<()> {
    let payload =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_bytes_msg(w, &payload).await
}

/// Read a 4-byte little-endian length-prefixed JSON payload.
pub async fn read_json_msg<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let payload = read_bytes_msg(r).await?;
    serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Write a 4-byte little-endian length-prefixed raw byte message.
pub async fn write_bytes_msg<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_GATEWAY_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "gateway frame exceeds MAX_GATEWAY_MSG",
        ));
    }
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&len).await?;
    w.write_all(payload).await?;
    w.flush().await
}

/// Read a 4-byte little-endian length-prefixed raw byte message.
pub async fn read_bytes_msg<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_GATEWAY_MSG {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "gateway frame exceeds MAX_GATEWAY_MSG",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Compress a raw pane protocol frame with fast zstd (level 1) when enabled.
pub fn encode_pane_chunk(raw: &[u8], compress_zstd: bool) -> io::Result<Vec<u8>> {
    if !compress_zstd {
        let mut out = Vec::with_capacity(1 + raw.len());
        out.push(0u8);
        out.extend_from_slice(raw);
        return Ok(out);
    }
    let compressed = zstd::bulk::compress(raw, 1).map_err(io::Error::other)?;
    let mut out = Vec::with_capacity(1 + compressed.len());
    out.push(1u8);
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Decode a possibly zstd-compressed pane protocol chunk.
pub fn decode_pane_chunk(chunk: &[u8]) -> io::Result<Vec<u8>> {
    let Some((&flag, rest)) = chunk.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty pane chunk",
        ));
    };
    match flag {
        0 => Ok(rest.to_vec()),
        1 => zstd::bulk::decompress(rest, MAX_GATEWAY_MSG).map_err(io::Error::other),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown pane chunk compression flag {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_pane_chunk_round_trips_and_shrinks_terminal_logs() {
        let log_burst = b"Compiling anterm-gateway v0.1.0\r\n".repeat(100);
        let compressed = encode_pane_chunk(&log_burst, true).unwrap();
        assert!(
            compressed.len() < log_burst.len() / 5,
            "zstd should heavily compress verbose terminal logs"
        );
        let decoded = decode_pane_chunk(&compressed).unwrap();
        assert_eq!(decoded, log_burst);

        let raw = encode_pane_chunk(b"hi", false).unwrap();
        assert_eq!(decode_pane_chunk(&raw).unwrap(), b"hi");
    }
}
