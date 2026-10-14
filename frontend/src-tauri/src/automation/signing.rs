use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn signature(secret: &str, timestamp: i64, raw_body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn signs_timestamp_and_exact_raw_body() {
        assert_eq!(
            super::signature("secret", 1, br#"{"event_id":"abc"}"#),
            "11c34fb55217ae46a95bd4ea1bc519997251634be64d58c345f179a452f62fe9"
        );
        assert_ne!(
            super::signature("secret", 1, br#"{"event_id":"abc"}"#),
            super::signature("secret", 1, br#"{ "event_id":"abc"}"#)
        );
    }
}
