use anyhow::{Result, ensure};
pub const APP_NAME: &str = "NS2GameShare";
pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 32 && !name.chars().any(char::is_control),
        "表示名は空白のみを避け、制御文字を含まない UTF-8 32 バイト以内にしてください（日本語は目安10文字）。"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn name_limits_are_utf8_bytes_not_characters() {
        assert!(super::validate_name("テストSwitch").is_ok());
        assert!(super::validate_name(&"あ".repeat(11)).is_err());
        for name in ["", "  ", "a\0b", "a\nb"] {
            assert!(super::validate_name(name).is_err());
        }
    }
}
