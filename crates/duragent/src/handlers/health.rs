use axum::http::StatusCode;

pub async fn livez() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

pub async fn readyz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_livez() {
        let (status, body) = livez().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn test_readyz() {
        let (status, body) = readyz().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }
}
