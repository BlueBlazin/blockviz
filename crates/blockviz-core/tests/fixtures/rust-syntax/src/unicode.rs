//! Non-ASCII text: identifiers, strings and comments keep their offsets.
/// Grüße an alle — “quoted”.
pub fn grüße(名前: &str) -> String {
    let 挨拶 = format!("こんにちは、{名前}さん 👋");
    挨拶
}

pub fn after_unicode() -> u32 {
    let ñ = 'ñ';
    ñ as u32
}
