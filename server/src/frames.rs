// Reads frames from worker replies.
use serde_json::Value;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// The largest desktop frame a guest may send.
pub const MAX_FRAME: usize = 768 * 1024;

/// Encodes a frame as padded base64. A JSON byte array costs about three times the
/// frame; base64 costs a third more, and is parsed as one string.
pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let value = u32::from(group[0]) << 16 | u32::from(group[1]) << 8 | u32::from(group[2]);
        for index in 0..4 {
            text.push(if index <= chunk.len() {
                ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char
            } else { '=' });
        }
    }
    text
}

/// Decodes padded base64, rejecting any other character or a bad length.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if text.len() % 4 != 0 { return None; }
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    for (index, group) in text.chunks(4).enumerate() {
        let last = index + 1 == text.len() / 4;
        let padding = group.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !last) { return None; }
        let mut value = 0u32;
        for byte in &group[..4 - padding] {
            value = value << 6 | ALPHABET.iter().position(|letter| letter == byte)? as u32;
        }
        value <<= 6 * padding as u32;
        bytes.extend_from_slice(&value.to_be_bytes()[1..4 - padding]);
    }
    Some(bytes)
}

/// Reads one bounded JPEG from a worker's `jpeg` field. New workers send base64; a
/// worker already installed in a guest still sends a byte array, which stays accepted
/// because updating it means reinstalling the guest.
pub fn jpeg(value: &Value) -> Option<Vec<u8>> {
    let bytes = match value {
        Value::String(text) if text.len() <= MAX_FRAME.div_ceil(3) * 4 => decode(text)?,
        Value::Array(items) if items.len() <= MAX_FRAME => items.iter()
            .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
            .collect::<Option<Vec<u8>>>()?,
        _ => return None,
    };
    (bytes.len() >= 4 && bytes.len() <= MAX_FRAME && bytes.starts_with(&[0xff, 0xd8, 0xff])
        && bytes.ends_with(&[0xff, 0xd9])).then_some(bytes)
}

/// Formats one frame of the multipart stream. The page reads the stream itself and needs
/// each part's length; it does not rely on the browser's own multipart drawing, which shows
/// a frame only once the next part starts (tests/experiments/mjpeg-draw.test.mjs).
pub fn part(frame: &[u8]) -> Vec<u8> {
    let mut part = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        frame.len()).into_bytes();
    part.extend_from_slice(frame);
    part.extend_from_slice(b"\r\n");
    part
}

/// Remembers the last frame a viewer received, so a repeat is not sent again.
#[derive(Default)]
pub struct Fresh(Option<Vec<u8>>);

impl Fresh {
    /// Accepts a frame that differs from the previous one and remembers it.
    pub fn accept(&mut self, frame: &[u8]) -> bool {
        if self.0.as_deref() == Some(frame) { return false; }
        self.0 = Some(frame.to_vec());
        true
    }
}

#[cfg(test)]
mod tests {
    // Checks the frame codec alone.
    use super::*;
    // Builds worker replies.
    use serde_json::json;

    /// Every length class round-trips and matches the standard encoding.
    #[test]
    fn base64_round_trips() {
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert_eq!(encode(b"M"), "TQ==");
        for length in 0..70usize {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 37 % 256) as u8).collect();
            assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
        }
        for bad in ["TWF", "TW=u", "T===", "TWF$", "TQ==TWFu"] { assert!(decode(bad).is_none(), "{bad}"); }
    }

    /// Both wire forms yield the same JPEG; anything else is refused.
    #[test]
    fn frames_accept_both_forms_and_only_jpegs() {
        let frame = vec![0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 0xff, 0xd9];
        assert_eq!(jpeg(&json!(encode(&frame))).unwrap(), frame);
        assert_eq!(jpeg(&json!(frame)).unwrap(), frame);
        assert!(jpeg(&json!(encode(b"not a jpeg at all"))).is_none());
        assert!(jpeg(&json!([255, 216, 255, 300])).is_none());
        assert!(jpeg(&json!({ "jpeg": 1 })).is_none());
        assert!(jpeg(&json!("A".repeat(MAX_FRAME * 2))).is_none());
    }

    #[test]
    fn every_part_states_its_length() {
        assert_eq!(part(b"one"),
            b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 3\r\n\r\none\r\n".to_vec());
    }

    #[test]
    fn a_repeated_frame_is_sent_once() {
        let mut fresh = Fresh::default();
        assert!(fresh.accept(b"one"));
        assert!(!fresh.accept(b"one"));
        assert!(fresh.accept(b"two"));
        assert!(fresh.accept(b"one"));
    }
}
