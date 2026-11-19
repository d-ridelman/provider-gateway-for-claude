use axum::http::HeaderMap;
use subtle::ConstantTimeEq;

pub fn authorized(headers: &HeaderMap, token: &str, open_mode: bool) -> bool {
    let supplied = supplied_key(headers);

    // Вкладка Code (GATEWAY_OPEN=1): сессия несёт OAuth-токен, выданный нашей же
    // заглушкой; принимаем любой непустой ключ, слушаем только 127.0.0.1.
    if open_mode && !supplied.is_empty() {
        return true;
    }
    if token.is_empty() {
        return false;
    }
    constant_time_eq(supplied.as_bytes(), token.as_bytes())
}

pub fn supplied_key(headers: &HeaderMap) -> String {
    let direct = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !direct.is_empty() {
        return direct.to_string();
    }
    let auth = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    match auth.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => auth[7..].trim().to_string(),
        _ => String::new(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.ct_eq(right).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn accepts_x_api_key_and_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("local-token"));
        assert!(authorized(&headers, "local-token", false));

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer local-token"),
        );
        assert!(authorized(&headers, "local-token", false));
        assert!(!authorized(&headers, "other", false));
    }

    #[test]
    fn open_mode_takes_any_non_empty_key() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("oauth-junk"));
        assert!(authorized(&headers, "expected", true));
        assert!(!authorized(&HeaderMap::new(), "expected", true));
    }
}
