//! The WebSocket framing `/events` needs (RFC 6455): the handshake key, and text, ping, pong and
//! close frames, as a server to bots and as a client of each phone's `/v1/events`.

use std::io::{self, Read, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// The largest message read; vphoned's own limit for a JSON frame.
const MESSAGE_MAX: usize = 1 << 20;

pub(crate) const TEXT: u8 = 0x1;
pub(crate) const CLOSE: u8 = 0x8;
pub(crate) const PING: u8 = 0x9;
pub(crate) const PONG: u8 = 0xA;

/// SHA-1 of `data` (FIPS 180-4), for the handshake only: never for security.
#[expect(clippy::many_single_char_names, reason = "FIPS 180-4's names")]
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bits = u64::try_from(data.len()).expect("a handshake key fits in u64") * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5A82_7999),
                20..40 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (o, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = x.to_be_bytes();
    }
    out
}

/// `Sec-WebSocket-Accept` for a client's `Sec-WebSocket-Key`.
pub(crate) fn accept(key: &str) -> String {
    BASE64.encode(sha1(
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    ))
}

/// A client's `Sec-WebSocket-Key`: 16 bytes in base64.
pub(crate) fn key(bytes: [u8; 16]) -> String {
    BASE64.encode(bytes)
}

/// Writes one unfragmented frame; a client masks it with `mask`, a server sends `None`.
pub(crate) fn write_frame(
    mut w: impl Write,
    opcode: u8,
    payload: &[u8],
    mask: Option<[u8; 4]>,
) -> io::Result<()> {
    let mut frame = vec![0x80 | opcode];
    let bit = if mask.is_some() { 0x80 } else { 0 };
    match u16::try_from(payload.len()) {
        Ok(n) if n < 126 => frame.push(bit | u8::try_from(n).expect("under 126")),
        Ok(n) => {
            frame.push(bit | 0x7E);
            frame.extend_from_slice(&n.to_be_bytes());
        }
        Err(_) => {
            frame.push(bit | 0x7F);
            let n = u64::try_from(payload.len()).expect("a length fits in u64");
            frame.extend_from_slice(&n.to_be_bytes());
        }
    }
    match mask {
        Some(m) => {
            frame.extend_from_slice(&m);
            frame.extend(payload.iter().zip(m.iter().cycle()).map(|(b, k)| b ^ k));
        }
        None => frame.extend_from_slice(payload),
    }
    w.write_all(&frame)
}

/// Reads one message: its opcode and payload, fragments joined, at most 1 MiB.
pub(crate) fn read_message(mut r: impl Read) -> io::Result<(u8, Vec<u8>)> {
    let too_big = || io::Error::new(io::ErrorKind::InvalidData, "WebSocket message over 1 MiB");
    let mut opcode = None;
    let mut message = Vec::new();
    loop {
        let mut h = [0u8; 2];
        r.read_exact(&mut h)?;
        let fin = h[0] & 0x80 != 0;
        let op = h[0] & 0x0F;
        let len = match h[1] & 0x7F {
            126 => {
                let mut n = [0u8; 2];
                r.read_exact(&mut n)?;
                u64::from(u16::from_be_bytes(n))
            }
            127 => {
                let mut n = [0u8; 8];
                r.read_exact(&mut n)?;
                u64::from_be_bytes(n)
            }
            n => u64::from(n),
        };
        let len = usize::try_from(len).map_err(|_| too_big())?;
        if message
            .len()
            .checked_add(len)
            .is_none_or(|n| n > MESSAGE_MAX)
        {
            return Err(too_big());
        }
        let mut mask = [0u8; 4];
        if h[1] & 0x80 != 0 {
            r.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload)?;
        payload
            .iter_mut()
            .zip(mask.iter().cycle())
            .for_each(|(b, k)| *b ^= k);
        // A control frame may arrive between the fragments of a message. A close ends the
        // message; a ping or pong there is dropped (ponytail: its pong is not sent, which only
        // matters to a peer that fragments and pings at once).
        if op >= CLOSE {
            if opcode.is_none() || op == CLOSE {
                return Ok((op, payload));
            }
            continue;
        }
        opcode.get_or_insert(op);
        message.extend_from_slice(&payload);
        if fin {
            return Ok((opcode.unwrap_or(TEXT), message));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_is_the_rfc_example() {
        // RFC 6455 section 1.3.
        assert_eq!(
            accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // FIPS 180 examples: one block, and two blocks.
        let hex = |d: [u8; 20]| {
            d.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
                s
            })
        };
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn frames_round_trip_masked_fragmented_and_long() {
        let mut buf = Vec::new();
        write_frame(&mut buf, TEXT, b"hello", Some([1, 2, 3, 4])).unwrap();
        assert_eq!(read_message(&buf[..]).unwrap(), (TEXT, b"hello".to_vec()));
        let long = vec![b'x'; 70_000];
        buf.clear();
        write_frame(&mut buf, TEXT, &long, None).unwrap();
        assert_eq!(read_message(&buf[..]).unwrap().1.len(), 70_000);
        // "he" + a ping between the fragments + "llo".
        let frag = [0x01, 2, b'h', b'e', 0x89, 0, 0x80, 3, b'l', b'l', b'o'];
        assert_eq!(read_message(&frag[..]).unwrap(), (TEXT, b"hello".to_vec()));
        assert_eq!(read_message(&[0x89, 0][..]).unwrap(), (PING, vec![]));
        let huge = [0x81, 127, 0, 0, 0, 0, 0, 0x20, 0, 0];
        assert!(read_message(&huge[..]).is_err(), "over 1 MiB is refused");
    }
}
