/// Frames a message with its length.
pub fn encode(message: &str) -> Vec<u8> {
    let mut out = (message.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(message.as_bytes());
    out
}
