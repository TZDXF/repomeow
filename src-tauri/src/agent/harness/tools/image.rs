//! 图片魔数探测与 base64 编码:对齐 `packages/agent/src/harness/tools/image.ts`。

use base64::Engine as _;

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// 探测受支持的图片 MIME(jpeg/png/gif/webp/bmp;APNG 不算 png)
/// (对齐 TS `detectSupportedImageMimeType`)。
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    if starts_with(buffer, &[0xFF, 0xD8, 0xFF]) {
        // SOF3(无损 JPEG)不是可发送的基线 JPEG。
        return if buffer.get(3) == Some(&0xF7) {
            None
        } else {
            Some("image/jpeg")
        };
    }
    if starts_with(buffer, &PNG_SIGNATURE) {
        return if is_png(buffer) && !is_animated_png(buffer) {
            Some("image/png")
        } else {
            None
        };
    }
    if starts_with_ascii(buffer, 0, "GIF") {
        return Some("image/gif");
    }
    if starts_with_ascii(buffer, 0, "RIFF") && starts_with_ascii(buffer, 8, "WEBP") {
        return Some("image/webp");
    }
    if starts_with_ascii(buffer, 0, "BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }
    None
}

/// 手写 base64(对齐蓝本 `encodeBase64`;与标准引擎输出一致)。
pub fn encode_base64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(bytes)
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_uint32_be(buffer, PNG_SIGNATURE.len()) == 13
        && starts_with_ascii(buffer, 12, "IHDR")
}

fn is_animated_png(buffer: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= buffer.len() {
        let chunk_length = read_uint32_be(buffer, offset) as usize;
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset, "acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset, "IDAT") {
            return false;
        }
        let next_offset = offset + 8 + chunk_length + 4;
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }
    let declared_file_size = read_uint32_le(buffer, 2);
    let pixel_data_offset = read_uint32_le(buffer, 10);
    let dib_header_size = read_uint32_le(buffer, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let (color_planes, bits_per_pixel) = if dib_header_size == 12 {
        (read_uint16_le(buffer, 22), read_uint16_le(buffer, 24))
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        (read_uint16_le(buffer, 26), read_uint16_le(buffer, 28))
    } else {
        return false;
    };
    color_planes == 1 && [1u32, 4, 8, 16, 24, 32].contains(&bits_per_pixel)
}

fn read_uint16_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(buffer.get(offset).copied().unwrap_or(0))
        | (u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 8)
}

fn read_uint32_be(buffer: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        buffer.get(offset).copied().unwrap_or(0),
        buffer.get(offset + 1).copied().unwrap_or(0),
        buffer.get(offset + 2).copied().unwrap_or(0),
        buffer.get(offset + 3).copied().unwrap_or(0),
    ])
}

fn read_uint32_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buffer.get(offset).copied().unwrap_or(0),
        buffer.get(offset + 1).copied().unwrap_or(0),
        buffer.get(offset + 2).copied().unwrap_or(0),
        buffer.get(offset + 3).copied().unwrap_or(0),
    ])
}

fn starts_with(buffer: &[u8], prefix: &[u8]) -> bool {
    buffer.len() >= prefix.len() && buffer[..prefix.len()] == *prefix
}

fn starts_with_ascii(buffer: &[u8], offset: usize, text: &str) -> bool {
    let bytes = text.as_bytes();
    if buffer.len() < offset + bytes.len() {
        return false;
    }
    buffer[offset..offset + bytes.len()] == *bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        // IHDR length 13。
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes
    }

    #[test]
    fn detects_png() {
        assert_eq!(detect_supported_image_mime_type(&png_header()), Some("image/png"));
    }

    #[test]
    fn rejects_apng() {
        // 真实 chunk 布局:signature + IHDR(len+type+data+crc) + acTL。
        let mut bytes = png_header();
        bytes.extend_from_slice(&[0u8; 13]); // IHDR data
        bytes.extend_from_slice(&4u32.to_be_bytes()); // IHDR crc
        bytes.extend_from_slice(&1u32.to_be_bytes()); // acTL length
        bytes.extend_from_slice(b"acTL");
        bytes.extend_from_slice(&[0u8; 8]);
        assert_eq!(detect_supported_image_mime_type(&bytes), None);
    }

    #[test]
    fn detects_jpeg_and_rejects_lossless() {
        assert_eq!(
            detect_supported_image_mime_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(detect_supported_image_mime_type(&[0xFF, 0xD8, 0xFF, 0xF7]), None);
    }

    #[test]
    fn detects_gif_webp_bmp() {
        assert_eq!(detect_supported_image_mime_type(b"GIF89a"), Some("image/gif"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(detect_supported_image_mime_type(&webp), Some("image/webp"));
        // BMP:header size 40,width/height 后 planes 1、bpp 24(对齐字段偏移)。
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&100u32.to_le_bytes()); // file size
        bmp.extend_from_slice(&[0u8; 4]); // reserved
        bmp.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
        bmp.extend_from_slice(&40u32.to_le_bytes()); // dib header size
        bmp.extend_from_slice(&10u32.to_le_bytes()); // width
        bmp.extend_from_slice(&10u32.to_le_bytes()); // height
        bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
        bmp.extend_from_slice(&24u16.to_le_bytes()); // bpp
        bmp.extend_from_slice(&[0u8; 12]);
        assert_eq!(detect_supported_image_mime_type(&bmp), Some("image/bmp"));
        assert_eq!(detect_supported_image_mime_type(b"BMnope"), None);
    }

    #[test]
    fn unknown_bytes_are_none() {
        assert_eq!(detect_supported_image_mime_type(b"hello"), None);
        assert_eq!(detect_supported_image_mime_type(&[]), None);
    }

    #[test]
    fn base64_matches_standard() {
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        assert_eq!(encode_base64(&[0xFB, 0xFF]), "+/8=");
        assert_eq!(encode_base64(&[]), "");
    }
}
