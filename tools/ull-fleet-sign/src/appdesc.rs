//! The ESP-IDF application descriptor at the start of an app image.
//!
//! Layout: 24-byte image header, 8-byte first segment header, then
//! `esp_app_desc_t` (magic, secure_version, reserved, version[32],
//! project_name[32], time[16], date[16], idf_ver[32], app_elf_sha256[32], ...).

use anyhow::{bail, ensure};

pub const IMAGE_MAGIC: u8 = 0xE9;
pub const APP_DESC_OFFSET: usize = 32;
pub const APP_DESC_MAGIC: u32 = 0xABCD_5432;
/// Bytes of the image needed to read every field below.
pub const PREFIX_LEN: usize = APP_DESC_OFFSET + 144;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDesc {
    pub secure_version: u32,
    pub version: String,
    pub project_name: String,
    pub time: String,
    pub date: String,
}

impl AppDesc {
    pub fn parse(image: &[u8]) -> anyhow::Result<Self> {
        ensure!(
            image.len() >= PREFIX_LEN,
            "image is too short to hold an ESP app descriptor"
        );
        if image[0] != IMAGE_MAGIC {
            bail!(
                "not an ESP application image (first byte {:#04x}, expected 0xe9); \
                 upload the output of `espflash save-image` without --merge",
                image[0]
            );
        }
        let word =
            |offset: usize| u32::from_le_bytes(image[offset..offset + 4].try_into().unwrap());
        let desc = APP_DESC_OFFSET;
        ensure!(
            word(desc) == APP_DESC_MAGIC,
            "ESP app descriptor magic missing; is this an esp-hal app image?"
        );

        let version = c_string(&image[desc + 16..desc + 48])?;
        ensure!(!version.is_empty(), "app descriptor has an empty version");
        Ok(Self {
            secure_version: word(desc + 4),
            version,
            project_name: c_string(&image[desc + 48..desc + 80])?,
            time: c_string(&image[desc + 80..desc + 96])?,
            date: c_string(&image[desc + 96..desc + 112])?,
        })
    }
}

fn c_string(field: &[u8]) -> anyhow::Result<String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    match std::str::from_utf8(&field[..end]) {
        Ok(text) => Ok(text.to_string()),
        Err(_) => bail!("app descriptor text field is not UTF-8"),
    }
}

/// Builds a minimal image prefix for tests in this and dependent crates.
pub fn test_image(project: &str, version: &str, secure_version: u32, body_len: usize) -> Vec<u8> {
    let mut image = vec![0u8; PREFIX_LEN + body_len];
    image[0] = IMAGE_MAGIC;
    let desc = APP_DESC_OFFSET;
    image[desc..desc + 4].copy_from_slice(&APP_DESC_MAGIC.to_le_bytes());
    image[desc + 4..desc + 8].copy_from_slice(&secure_version.to_le_bytes());
    image[desc + 16..desc + 16 + version.len()].copy_from_slice(version.as_bytes());
    image[desc + 48..desc + 48 + project.len()].copy_from_slice(project.as_bytes());
    image[desc + 80..desc + 88].copy_from_slice(b"12:00:00");
    image[desc + 96..desc + 107].copy_from_slice(b"Nov 22 2026");
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields() {
        let image = test_image("sht3x-ssd1306", "0.1.0+abc-1795387024", 1_795_387_024, 10);
        let desc = AppDesc::parse(&image).unwrap();
        assert_eq!(desc.project_name, "sht3x-ssd1306");
        assert_eq!(desc.version, "0.1.0+abc-1795387024");
        assert_eq!(desc.secure_version, 1_795_387_024);
        assert_eq!(desc.date, "Nov 22 2026");
    }

    #[test]
    fn rejects_non_images() {
        assert!(AppDesc::parse(b"short").is_err());
        let mut image = test_image("p", "1", 1, 0);
        image[0] = 0x7f;
        assert!(AppDesc::parse(&image).is_err());
        let mut image = test_image("p", "1", 1, 0);
        image[APP_DESC_OFFSET] = 0;
        assert!(AppDesc::parse(&image).is_err());
        assert!(AppDesc::parse(&test_image("p", "", 1, 0)).is_err());
    }

    /// The real http-ota image built during the 2026-11-22 hardware test.
    #[test]
    fn parses_real_image_when_available() {
        let path = std::env::var("ULL_TEST_IMAGE").unwrap_or_default();
        if let Ok(image) = std::fs::read(path) {
            let desc = AppDesc::parse(&image).unwrap();
            assert_eq!(desc.project_name, "http-ota");
        }
    }
}
