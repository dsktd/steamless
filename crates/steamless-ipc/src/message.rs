//! Requests and responses, u32 big-endian length + bincode on the wire.

use std::io::Read;
use std::io::Write;

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;

use crate::Result;

/// Larger frames are refused rather than allocated.
const MAX_FRAME: u32 = 1 << 20;

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub enum IpcRequest {
    Status,
    /// Re-read the config file and re-apply the active profile.
    Reload,
    /// Switch every controller to the named profile.
    Activate(String),
}

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub enum IpcResponse {
    Ok,
    Status(IpcStatus),
    Error(String),
}

#[derive(Encode, Decode, Clone, Debug, Default, PartialEq)]
pub struct IpcStatus {
    pub profile: String,
    pub controllers: Vec<IpcController>,
}

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub struct IpcController {
    pub id: u8,
    pub path: String,
    /// Percent, once the controller has reported it.
    pub battery: Option<u8>,
    pub lizard: bool,
}

pub fn write(
    mut writer: impl Write,
    message: &impl Encode,
) -> Result<()> {
    let buf = bincode::encode_to_vec(message, bincode::config::standard())?;
    writer.write_all(&(buf.len() as u32).to_be_bytes())?;
    writer.write_all(&buf)?;
    writer.flush()?;
    Ok(())
}

pub fn read<T: Decode<()>>(mut reader: impl Read) -> Result<T> {
    let mut len_buf = [0_u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(anyhow!("Frame of {len} bytes is too large"));
    }
    let mut buf = Vec::with_capacity(len as usize);
    buf.resize(len as usize, 0);
    reader.read_exact(&mut buf)?;
    let (message, _len) = bincode::decode_from_slice(&buf, bincode::config::standard())?;
    Ok(message)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_framing_round_trip() {
        let mut buf = Vec::new();
        write(&mut buf, &IpcRequest::Activate("browser".into())).unwrap();
        write(&mut buf, &IpcRequest::Status).unwrap();
        let status = IpcResponse::Status(IpcStatus {
            profile: "pad".into(),
            controllers: vec![IpcController {
                id: 3,
                path: "/dev/hidraw3".into(),
                battery: Some(80),
                lizard: false,
            }],
        });
        write(&mut buf, &status).unwrap();
        // Variant, string length, "browser"
        assert_eq!(buf[..4], 9_u32.to_be_bytes());
        let mut reader = buf.as_slice();
        assert_eq!(
            read::<IpcRequest>(&mut reader).unwrap(),
            IpcRequest::Activate("browser".into())
        );
        assert_eq!(read::<IpcRequest>(&mut reader).unwrap(), IpcRequest::Status);
        assert_eq!(read::<IpcResponse>(&mut reader).unwrap(), status);
        assert!(read::<IpcRequest>(&mut reader).is_err());
    }
}
