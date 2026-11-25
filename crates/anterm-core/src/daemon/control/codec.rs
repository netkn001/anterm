use std::io::{self, Read, Write};

use crate::daemon::protocol::{MAX_FRAME, read_frame, write_frame};
use crate::util::{from_json, to_json};

use super::{ControlEvent, ControlHello, ControlHelloOk, ControlReply, ControlRequest, kind};

pub(super) const CONTROL_HEADER: usize = 12;

pub(super) fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

pub(super) fn encode_body(req_id: u64, json: &[u8], blob: &[u8]) -> io::Result<Vec<u8>> {
    let json_n = u32::try_from(json.len())
        .map_err(|_| invalid("control JSON exceeds the u32 length prefix"))?;
    let mut out = Vec::with_capacity(CONTROL_HEADER + json.len() + blob.len());
    out.extend_from_slice(&req_id.to_le_bytes());
    out.extend_from_slice(&json_n.to_le_bytes());
    out.extend_from_slice(json);
    out.extend_from_slice(blob);
    Ok(out)
}

fn decode_body(payload: &[u8]) -> io::Result<(u64, &[u8], &[u8])> {
    if payload.len() < CONTROL_HEADER {
        return Err(invalid(format!(
            "control payload is {} bytes, shorter than the {CONTROL_HEADER}-byte header",
            payload.len()
        )));
    }
    let req_id = u64::from_le_bytes(
        payload[..8]
            .try_into()
            .expect("control header length verified above"),
    );
    let json_n = u32::from_le_bytes(
        payload[8..12]
            .try_into()
            .expect("control header length verified above"),
    ) as usize;
    let json_end = CONTROL_HEADER
        .checked_add(json_n)
        .ok_or_else(|| invalid("control json_len overflows the payload offset"))?;
    if json_end > payload.len() {
        return Err(invalid(format!(
            "control json_len {json_n} runs past the {}-byte payload",
            payload.len()
        )));
    }
    Ok((
        req_id,
        &payload[CONTROL_HEADER..json_end],
        &payload[json_end..],
    ))
}

fn decode_body_exact<'a>(payload: &'a [u8], what: &str) -> io::Result<(u64, &'a [u8])> {
    let (req_id, json, blob) = decode_body(payload)?;
    if !blob.is_empty() {
        return Err(invalid(format!(
            "{what} carries {} trailing bytes; only the *_BLOB kinds may",
            blob.len()
        )));
    }
    Ok((req_id, json))
}

fn require_nonzero(req_id: u64, what: &str) -> io::Result<()> {
    if req_id == 0 {
        return Err(invalid(format!(
            "{what} used req_id 0, which is reserved for server pushes"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlClientMsg {
    Hello(ControlHello),
    Request {
        req_id: u64,
        req: ControlRequest,
    },
    RequestBlob {
        req_id: u64,
        req: ControlRequest,
        blob: Vec<u8>,
    },
    Cancel {
        req_id: u64,
    },
}

impl ControlClientMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let (k, payload) = self.to_frame()?;
        write_frame(w, k, &payload)
    }

    pub fn to_frame(&self) -> io::Result<(u8, Vec<u8>)> {
        let (k, payload) = match self {
            ControlClientMsg::Hello(hello) => (kind::HELLO, to_json(hello)?),
            ControlClientMsg::Request { req_id, req } => {
                require_nonzero(*req_id, "CONTROL_REQUEST")?;
                (kind::REQUEST, encode_body(*req_id, &to_json(req)?, &[])?)
            }
            ControlClientMsg::RequestBlob { req_id, req, blob } => {
                require_nonzero(*req_id, "CONTROL_REQUEST_BLOB")?;
                (
                    kind::REQUEST_BLOB,
                    encode_body(*req_id, &to_json(req)?, blob)?,
                )
            }
            ControlClientMsg::Cancel { req_id } => {
                require_nonzero(*req_id, "CONTROL_CANCEL")?;
                (kind::CANCEL, encode_body(*req_id, &[], &[])?)
            }
        };
        if payload.len() > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "control frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
                    payload.len()
                ),
            ));
        }
        Ok((k, payload))
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        match k {
            kind::HELLO => Ok(ControlClientMsg::Hello(from_json(&payload)?)),
            kind::REQUEST => {
                let (req_id, json) = decode_body_exact(&payload, "CONTROL_REQUEST")?;
                require_nonzero(req_id, "CONTROL_REQUEST")?;
                Ok(ControlClientMsg::Request {
                    req_id,
                    req: from_json(json)?,
                })
            }
            kind::REQUEST_BLOB => {
                let (req_id, json, blob) = decode_body(&payload)?;
                require_nonzero(req_id, "CONTROL_REQUEST_BLOB")?;
                Ok(ControlClientMsg::RequestBlob {
                    req_id,
                    req: from_json(json)?,
                    blob: blob.to_vec(),
                })
            }
            kind::CANCEL => {
                let (req_id, json) = decode_body_exact(&payload, "CONTROL_CANCEL")?;
                require_nonzero(req_id, "CONTROL_CANCEL")?;
                if !json.is_empty() {
                    return Err(invalid("CONTROL_CANCEL must carry an empty JSON head"));
                }
                Ok(ControlClientMsg::Cancel { req_id })
            }
            other => Err(invalid(format!("unknown ControlClientMsg kind {other}"))),
        }
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlServerMsg {
    HelloOk(ControlHelloOk),
    Response {
        req_id: u64,
        reply: ControlReply,
    },
    ResponseBlob {
        req_id: u64,
        reply: ControlReply,
        blob: Vec<u8>,
    },
    Event(ControlEvent),
}

impl ControlServerMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let (k, payload) = self.to_frame()?;
        write_frame(w, k, &payload)
    }

    pub fn to_frame(&self) -> io::Result<(u8, Vec<u8>)> {
        let (k, payload) = match self {
            ControlServerMsg::HelloOk(ok) => (kind::HELLO_OK, to_json(ok)?),
            ControlServerMsg::Response { req_id, reply } => {
                require_nonzero(*req_id, "CONTROL_RESPONSE")?;
                (kind::RESPONSE, encode_body(*req_id, &to_json(reply)?, &[])?)
            }
            ControlServerMsg::ResponseBlob {
                req_id,
                reply,
                blob,
            } => {
                require_nonzero(*req_id, "CONTROL_RESPONSE_BLOB")?;
                (
                    kind::RESPONSE_BLOB,
                    encode_body(*req_id, &to_json(reply)?, blob)?,
                )
            }
            ControlServerMsg::Event(event) => (kind::EVENT, encode_body(0, &to_json(event)?, &[])?),
        };
        if payload.len() > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "control frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
                    payload.len()
                ),
            ));
        }
        Ok((k, payload))
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        match k {
            kind::HELLO_OK => Ok(ControlServerMsg::HelloOk(from_json(&payload)?)),
            kind::RESPONSE => {
                let (req_id, json) = decode_body_exact(&payload, "CONTROL_RESPONSE")?;
                require_nonzero(req_id, "CONTROL_RESPONSE")?;
                Ok(ControlServerMsg::Response {
                    req_id,
                    reply: from_json(json)?,
                })
            }
            kind::RESPONSE_BLOB => {
                let (req_id, json, blob) = decode_body(&payload)?;
                require_nonzero(req_id, "CONTROL_RESPONSE_BLOB")?;
                Ok(ControlServerMsg::ResponseBlob {
                    req_id,
                    reply: from_json(json)?,
                    blob: blob.to_vec(),
                })
            }
            kind::EVENT => {
                let (req_id, json) = decode_body_exact(&payload, "CONTROL_EVENT")?;
                if req_id != 0 {
                    return Err(invalid(format!(
                        "CONTROL_EVENT carried req_id {req_id}; pushes must use 0"
                    )));
                }
                Ok(ControlServerMsg::Event(from_json(json)?))
            }
            other => Err(invalid(format!("unknown ControlServerMsg kind {other}"))),
        }
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}
