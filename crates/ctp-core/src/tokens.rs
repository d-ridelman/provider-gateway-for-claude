use serde_json::Value;
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Грубая оценка токенов без словаря Claude: латиница ≈ 4 символа на токен,
/// CJK и кириллица — ближе к 1–2 символам. Точности tiktoken здесь не ждём;
/// число нужно, чтобы Claude не считал контекст нулевым.
pub fn estimate_tokens(text: &str) -> u64 {
    let mut wide = 0u64;
    let mut narrow = 0u64;
    for ch in text.chars() {
        if is_wide(ch) {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    let estimated = wide + narrow.div_ceil(4);
    estimated.max(1)
}

/// Точнее, чем эвристика: тот же BPE, которым пользуется Python-шлюз
/// (LiteLLM считает `gpt-4o`). Для модели Claude это всё равно оценка.
pub fn count_tokens(text: &str) -> u64 {
    match bpe() {
        Some(bpe) => bpe.encode_ordinary(text).len() as u64,
        None => estimate_tokens(text),
    }
}

/// Токены запроса Anthropic Messages. Картинки и PDF считаются по их размеру, как их
/// считает модель, а не по тексту base64: скриншот 1920×1140 — это ~1,5 тыс. токенов,
/// а его base64 токенизатор насчитал бы ~250 тыс., и Claude Code сжимал бы разговор зря.
pub fn count_json_tokens(value: &Value) -> u64 {
    let mut media = 0u64;
    let rest = without_media(value, &mut media);
    match serde_json::to_string(&rest) {
        Ok(text) => (count_tokens(&text) + media).max(1),
        Err(_) => media.max(1),
    }
}

pub fn estimate_json_tokens(value: &Value) -> u64 {
    let mut media = 0u64;
    let rest = without_media(value, &mut media);
    match serde_json::to_string(&rest) {
        Ok(text) => estimate_tokens(&text) + media,
        Err(_) => media.max(1),
    }
}

/// Токенов на картинку, если её размер неизвестен (ссылка, непонятный формат): столько
/// модель берёт за картинку максимального размера.
const IMAGE_FALLBACK: u64 = 1600;
/// Токенов на страницу PDF: текст страницы и её картинка.
const PDF_PAGE: u64 = 2000;

/// Копия без данных картинок и PDF (`source.data`); их токены — в `media`.
fn without_media(value: &Value, media: &mut u64) -> Value {
    match value {
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str);
            if let (Some(kind @ ("image" | "document")), Some(Value::Object(source))) =
                (kind, map.get("source"))
            {
                let source_kind = source.get("type").and_then(Value::as_str);
                let data = source.get("data").and_then(Value::as_str);
                let media_type = source
                    .get("media_type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let counted = match (source_kind, data) {
                    (Some("base64"), Some(data)) if kind == "image" => Some(image_tokens(data)),
                    (Some("base64"), Some(data)) if media_type == "application/pdf" => {
                        Some(pdf_tokens(data))
                    }
                    (Some("url"), _) if kind == "image" => Some(IMAGE_FALLBACK),
                    (Some("url"), _) => Some(PDF_PAGE),
                    // Текстовые документы и прочее считаются как текст.
                    _ => None,
                };
                if let Some(tokens) = counted {
                    *media += tokens;
                    let mut block = map.clone();
                    let mut source = source.clone();
                    source.insert("data".into(), Value::String(String::new()));
                    block.insert("source".into(), Value::Object(source));
                    return Value::Object(block);
                }
            }
            Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), without_media(item, media)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| without_media(item, media))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Токены картинки, как у Claude: больше 1568 px по длинной стороне или ~1,15 Мп она
/// уменьшается, а дальше ширина × высота / 750.
fn image_tokens(data: &str) -> u64 {
    // Заголовок JPEG бывает далеко (EXIF): хватит первых 96 КБ.
    let head = decode_base64_prefix(data, 96 * 1024);
    let Some((width, height)) = image_size(&head) else {
        return IMAGE_FALLBACK;
    };
    let (w, h) = (f64::from(width), f64::from(height));
    let scale = (1568.0 / w.max(h))
        .min((1_150_000.0 / (w * h)).sqrt())
        .min(1.0);
    ((w * scale) * (h * scale) / 750.0).ceil().max(1.0) as u64
}

/// Страницы PDF (`/Type /Page`, не `/Pages`) × токенов на страницу.
fn pdf_tokens(data: &str) -> u64 {
    let bytes = decode_base64_prefix(data, usize::MAX);
    let mut pages = 0u64;
    let mut i = 0;
    while let Some(at) = bytes[i..].windows(5).position(|w| w == b"/Type") {
        let mut j = i + at + 5;
        while bytes.get(j) == Some(&b' ') {
            j += 1;
        }
        if bytes[j..].starts_with(b"/Page") && bytes.get(j + 5) != Some(&b's') {
            pages += 1;
        }
        i = j;
    }
    pages.max(1) * PDF_PAGE
}

/// Ширина и высота PNG, JPEG, GIF или WebP по заголовку.
fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u32::from(*b.get(i)?) << 8 | u32::from(*b.get(i + 1)?));
    let le16 = |i: usize| Some(u32::from(*b.get(i)?) | u32::from(*b.get(i + 1)?) << 8);
    let le24 = |i: usize| Some(le16(i)? | u32::from(*b.get(i + 2)?) << 16);
    let be32 = |i: usize| Some(be16(i)? << 16 | be16(i + 2)?);
    let size = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        (be32(16)?, be32(20)?)
    } else if b.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        match b.get(12..16)? {
            b"VP8 " => (le16(26)? & 0x3fff, le16(28)? & 0x3fff),
            b"VP8L" => {
                let bits = le16(21)? | le16(23)? << 16;
                ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
            }
            b"VP8X" => (le24(24)? + 1, le24(27)? + 1),
            _ => return None,
        }
    } else if b.starts_with(&[0xFF, 0xD8]) {
        // Маркеры JPEG до кадра SOFn (кроме DHT C4, JPG C8, DAC CC).
        let mut i = 2;
        loop {
            if *b.get(i)? != 0xFF {
                return None;
            }
            let marker = *b.get(i + 1)?;
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                break (be16(i + 7)?, be16(i + 5)?);
            }
            i += 2 + be16(i + 2)? as usize;
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// Начало base64 (до `limit` байт); неверные символы пропускаются.
fn decode_base64_prefix(data: &str, limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in data.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        buffer = (buffer << 6 | u32::from(value)) & 0xFF_FFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().ok()).as_ref()
}

fn is_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0400..=0x04FF      // кириллица
        | 0x0590..=0x05FF    // иврит
        | 0x0600..=0x06FF    // арабский
        | 0x0900..=0x097F    // деванагари
        | 0x0E00..=0x0E7F    // тайский
        | 0x3040..=0x30FF    // хирагана, катакана
        | 0x3400..=0x4DBF    // CJK extension A
        | 0x4E00..=0x9FFF    // CJK
        | 0xAC00..=0xD7AF    // хангыль
        | 0xF900..=0xFAFF    // CJK compatibility
        | 0xFF00..=0xFFEF    // полноширинные формы
        | 0x1F300..=0x1FAFF  // эмодзи
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn token_counts_are_positive_and_differ_from_estimates() {
        assert_eq!(count_tokens(""), 0);
        assert!(count_tokens("hello world") > 0);
        assert!(count_tokens("привет мир") > 0);
    }

    fn base64(bytes: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n =
                chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
            for k in 0..=chunk.len() {
                out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
            }
            out.push_str(&"=".repeat(3 - chunk.len()));
        }
        out
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(width.to_be_bytes());
        b.extend(height.to_be_bytes());
        // Тело картинки: много байт, как у настоящего скриншота.
        b.extend(std::iter::repeat_n(0x5a, 300_000));
        b
    }

    #[test]
    fn images_count_by_size_not_by_base64_text() {
        let screenshot = base64(&png(1920, 1140));
        let body = json!({"messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": screenshot}},
            {"type": "text", "text": "hi"}
        ]}]});
        let tokens = count_json_tokens(&body);
        // Уменьшена до ~1,15 Мп → ~1,5 тыс., а не сотни тысяч за base64.
        assert!((1400..1700).contains(&tokens), "{tokens}");
        assert!(estimate_json_tokens(&body) < 1700);
        // Маленькая — по размеру, без уменьшения: 200×100 / 750 → 27.
        assert_eq!(image_tokens(&base64(&png(200, 100))), 27);
        // Неизвестный формат и ссылка — как картинка максимального размера.
        assert_eq!(image_tokens(&base64(b"not an image")), IMAGE_FALLBACK);
        let url = json!({"type": "image", "source": {"type": "url", "url": "https://x/y.png"}});
        assert!(count_json_tokens(&url) >= IMAGE_FALLBACK);
    }

    #[test]
    fn image_formats_give_their_size() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend(640u16.to_le_bytes());
        gif.extend(480u16.to_le_bytes());
        assert_eq!(image_size(&gif), Some((640, 480)));

        // JPEG: APP0, потом SOF0 с высотой и шириной.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend([0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend(768u16.to_be_bytes());
        jpeg.extend(1024u16.to_be_bytes());
        assert_eq!(image_size(&jpeg), Some((1024, 768)));

        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.extend([0u8; 8]);
        webp.extend([0x7F, 0x07, 0x00]); // ширина − 1 = 1919
        webp.extend([0x73, 0x04, 0x00]); // высота − 1 = 1139
        assert_eq!(image_size(&webp), Some((1920, 1140)));
        assert_eq!(image_size(&png(3, 4)[..24]), Some((3, 4)));
        assert_eq!(image_size(b"\xFF\xD8\xFF"), None);
    }

    #[test]
    fn pdf_counts_pages() {
        let pdf = b"%PDF-1.4 1 0 obj << /Type /Pages /Kids [2 0 R 3 0 R] >> \
                    2 0 obj << /Type /Page >> 3 0 obj <</Type/Page>>";
        let doc = json!({"type": "document", "source": {"type": "base64",
            "media_type": "application/pdf", "data": base64(pdf)}});
        let tokens = count_json_tokens(&doc);
        assert!(
            (2 * PDF_PAGE..2 * PDF_PAGE + 100).contains(&tokens),
            "{tokens}"
        );
        // Текстовый документ считается как текст.
        let text = json!({"type": "document", "source": {"type": "text",
            "media_type": "text/plain", "data": "hello world"}});
        assert!(count_json_tokens(&text) < 100);
    }

    #[test]
    fn estimates_are_positive_and_scale() {
        assert_eq!(estimate_tokens(""), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("привет"), 6);
        assert_eq!(estimate_tokens("你好"), 2);
    }
}
