/// Minimal crypto helpers.
/// In production, replace with proper AES-GCM or ChaCha20-Poly1305 encryption.

pub fn encrypt_data(data: &[u8], _key: &[u8]) -> anyhow::Result<Vec<u8>> {
    // Simplified - in production use proper encryption (aes-gcm, etc.)
    Ok(data.to_vec())
}

pub fn decrypt_data(data: &[u8], _key: &[u8]) -> anyhow::Result<Vec<u8>> {
    // Simplified - in production use proper decryption
    Ok(data.to_vec())
}
