//! Bounded Modbus framing and validation of common standard PDU layouts.
use std::{io, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;

pub const MAX_ADU: usize = 260;

#[derive(Debug, Clone)]
pub struct Frame {
    bytes: [u8; MAX_ADU],
    len: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// The caller supplies at least the eight-byte MBAP/function prefix.
fn tcp_length(prefix: &[u8], response: bool) -> io::Result<(usize, bool)> {
    if prefix[2..4] != [0, 0] {
        return Err(invalid("nonzero MBAP protocol identifier"));
    }
    let length = u16::from_be_bytes([prefix[4], prefix[5]]) as usize;
    if !(2..=254).contains(&length) {
        return Err(invalid("MBAP length outside 2..=254"));
    }
    if response && prefix[7] & 0x80 != 0 {
        Ok((9, length != 3))
    } else {
        Ok((6 + length, false))
    }
}

/// One bounded receive buffer per TCP connection. A read may contain a partial
/// frame, a complete frame, or several frames; surplus bytes remain here. Drop
/// this reader together with its stream after cancellation or a protocol error.
pub struct FrameReader {
    bytes: [u8; MAX_ADU],
    start: usize,
    end: usize,
}

impl Default for FrameReader {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_ADU],
            start: 0,
            end: 0,
        }
    }
}

impl FrameReader {
    async fn fill<R: AsyncRead + Unpin>(&mut self, reader: &mut R) -> io::Result<usize> {
        if self.start > 0 {
            self.bytes.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let read = reader.read(&mut self.bytes[self.end..]).await?;
        self.end += read;
        Ok(read)
    }

    /// Idle budget ends with the first read, which already consumes available
    /// data. The separate frame budget covers all subsequent partial reads.
    pub async fn read_request<R: AsyncRead + Unpin>(
        &mut self,
        reader: &mut R,
        idle: Duration,
        frame: Duration,
    ) -> io::Result<Option<Frame>> {
        if self.start == self.end && timeout(idle, self.fill(reader)).await?? == 0 {
            return Ok(None);
        }
        Ok(timeout(frame, self.read(reader, false))
            .await??
            .map(|(frame, _)| frame))
    }

    pub async fn read<R: AsyncRead + Unpin>(
        &mut self,
        reader: &mut R,
        response: bool,
    ) -> io::Result<Option<(Frame, bool)>> {
        loop {
            let available = &self.bytes[self.start..self.end];
            if available.len() >= 8 {
                let (len, repaired) = tcp_length(available, response)?;
                if available.len() >= len {
                    let mut frame = Frame {
                        bytes: [0; MAX_ADU],
                        len,
                    };
                    frame.bytes[..len].copy_from_slice(&available[..len]);
                    if repaired {
                        frame.bytes[4..6].copy_from_slice(&3_u16.to_be_bytes());
                    }
                    self.start += len;
                    return Ok(Some((frame, repaired)));
                }
            }
            if self.fill(reader).await? == 0 {
                return if self.start == self.end {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "incomplete Modbus frame",
                    ))
                };
            }
        }
    }
}

impl Frame {
    pub fn new(tid: u16, unit: u8, pdu: &[u8]) -> io::Result<Self> {
        if pdu.is_empty() || pdu.len() > 253 {
            return Err(invalid("PDU length must be 1..=253"));
        }
        let mut result = Self {
            bytes: [0; MAX_ADU],
            len: 7 + pdu.len(),
        };
        result.set_tid(tid);
        result.bytes[4..6].copy_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
        result.bytes[6] = unit;
        result.bytes[7..result.len].copy_from_slice(pdu);
        Ok(result)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn tid(&self) -> u16 {
        u16::from_be_bytes([self.bytes[0], self.bytes[1]])
    }
    pub fn set_tid(&mut self, tid: u16) {
        self.bytes[..2].copy_from_slice(&tid.to_be_bytes());
    }
    pub fn unit(&self) -> u8 {
        self.bytes[6]
    }
    pub fn set_unit(&mut self, unit: u8) {
        self.bytes[6] = unit;
    }
    pub fn function(&self) -> u8 {
        self.bytes[7]
    }
    pub fn pdu(&self) -> &[u8] {
        &self.bytes[7..self.len]
    }
    pub fn exception(&self, code: u8) -> Self {
        Self::new(self.tid(), self.unit(), &[self.function() | 0x80, code])
            .expect("fixed exception size")
    }
    pub fn is_read_only(&self) -> bool {
        // Only the four unambiguous standard read functions are retried.
        // Vendor extensions and encapsulated functions can have side effects.
        matches!(self.function(), 1..=4)
    }

    /// Validate common standard requests before they can mutate a device. Other
    /// functions (including vendor extensions) remain opaque, bounded PDUs.
    /// Errors are Modbus exception codes, not transport errors.
    pub fn validate_request(&self) -> Result<(), u8> {
        let pdu = self.pdu();
        let word = |i| u16::from_be_bytes([pdu[i], pdu[i + 1]]);
        let range = |offset: usize, max: u16| -> Result<usize, u8> {
            let start = u32::from(word(offset));
            let quantity = word(offset + 2);
            if quantity == 0 || quantity > max {
                return Err(3);
            }
            if start + u32::from(quantity) > 65536 {
                return Err(2);
            }
            Ok(usize::from(quantity))
        };
        match self.function() {
            1..=4 => {
                if pdu.len() != 5 {
                    return Err(3);
                }
                range(1, if self.function() <= 2 { 2000 } else { 125 })?;
            }
            5 | 6 if pdu.len() != 5 || self.function() == 5 && !matches!(word(3), 0 | 0xff00) => {
                return Err(3);
            }
            15 | 16 => {
                if pdu.len() < 6 {
                    return Err(3);
                }
                let quantity = range(1, if self.function() == 15 { 1968 } else { 123 })?;
                let bytes = if self.function() == 15 {
                    quantity.div_ceil(8)
                } else {
                    quantity * 2
                };
                if usize::from(pdu[5]) != bytes || pdu.len() != 6 + bytes {
                    return Err(3);
                }
            }
            17 if pdu.len() != 1 => return Err(3),
            22 if pdu.len() != 7 => return Err(3),
            23 => {
                if pdu.len() < 10 {
                    return Err(3);
                }
                range(1, 125)?;
                let bytes = range(5, 121)? * 2;
                if usize::from(pdu[9]) != bytes || pdu.len() != 10 + bytes {
                    return Err(3);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// On failure or cancellation the stream MUST be discarded. The returned bool
    /// requests a connection reset for a repaired exception (possible late padding).
    pub async fn read<R: AsyncRead + Unpin>(
        reader: &mut R,
        response: bool,
    ) -> io::Result<(Self, bool)> {
        let mut frame = Self {
            bytes: [0; MAX_ADU],
            len: 0,
        };
        reader.read_exact(&mut frame.bytes[..8]).await?;
        let (len, repaired) = tcp_length(&frame.bytes[..8], response)?;
        frame.len = len;
        reader.read_exact(&mut frame.bytes[8..frame.len]).await?;
        if repaired {
            frame.bytes[4..6].copy_from_slice(&3_u16.to_be_bytes());
        }
        Ok((frame, repaired))
    }
    pub fn validate_response(&self, request: &Self) -> io::Result<()> {
        if self.tid() != request.tid() {
            return Err(invalid("backend transaction ID mismatch"));
        }
        if self.unit() != request.unit() {
            return Err(invalid("backend unit ID mismatch"));
        }
        if self.function() != request.function() && self.function() != request.function() | 0x80 {
            return Err(invalid("backend function mismatch"));
        }
        let pdu = self.pdu();
        let sent = request.pdu();
        if self.function() & 0x80 != 0 {
            // Nonstandard nonzero exception codes are legitimate vendor extensions.
            if pdu.len() != 2 || pdu[1] == 0 {
                return Err(invalid("malformed exception PDU"));
            }
            return Ok(());
        }
        request
            .validate_request()
            .map_err(|_| invalid("invalid original request"))?;
        match self.function() {
            1..=4 | 23 => {
                let quantity = usize::from(u16::from_be_bytes([sent[3], sent[4]]));
                let expected = if self.function() <= 2 {
                    quantity.div_ceil(8)
                } else {
                    quantity * 2
                };
                if pdu.len() != 2 + expected || usize::from(pdu[1]) != expected {
                    return Err(invalid(
                        "response byte count differs from requested quantity",
                    ));
                }
            }
            5 | 6 | 22 if pdu != sent => {
                return Err(invalid("write confirmation does not echo the request"));
            }
            15 | 16 if pdu != &sent[..5] => {
                return Err(invalid("write confirmation address or quantity mismatch"));
            }
            17 if pdu.len() < 4 || pdu.len() != 2 + usize::from(pdu[1]) => {
                return Err(invalid("invalid report server ID response length"));
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub fn rtu_request(frame: &Frame) -> ([u8; 256], usize) {
    let mut bytes = [0; 256];
    let body = &frame.as_bytes()[6..];
    bytes[..body.len()].copy_from_slice(body);
    bytes[body.len()..body.len() + 2].copy_from_slice(&crc16(body).to_le_bytes());
    (bytes, body.len() + 2)
}

/// RTU has no generic length field. Parse supported function layouts, never
/// treat an accidental CRC match in a payload prefix as a frame boundary.
pub fn rtu_supported(function: u8) -> bool {
    matches!(function, 1..=6 | 15..=17 | 22 | 23)
}
pub async fn read_rtu<R: AsyncRead + Unpin>(reader: &mut R, tid: u16) -> io::Result<Frame> {
    let mut bytes = [0; 256];
    reader.read_exact(&mut bytes[..3]).await?;
    let len = match bytes[1] {
        f if f & 0x80 != 0 => 5,
        1..=4 | 17 | 23 => 5 + usize::from(bytes[2]),
        5 | 6 | 15 | 16 => 8,
        22 => 10,
        _ => return Err(invalid("unsupported RTU function layout")),
    };
    if len > bytes.len() {
        return Err(invalid("RTU frame exceeds 256 bytes"));
    }
    reader.read_exact(&mut bytes[3..len]).await?;
    if crc16(&bytes[..len]) != 0 {
        return Err(invalid("RTU CRC mismatch"));
    }
    Frame::new(tid, bytes[0], &bytes[1..len - 2])
}
