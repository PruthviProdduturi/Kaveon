use axum::response::Html;

pub async fn dashboard() -> Html<&'static str> {
    Html(include_str!("ui.html"))
}

pub async fn msal_script() -> impl axum::response::IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("vendor/msal-browser.min.js"),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn dashboard_includes_accessible_query_loading_error_and_identity_states() {
        let shell = include_str!("ui.html");
        for expected in [
            "id=\"history-status\" role=\"status\" aria-live=\"polite\"",
            "Loading query history…",
            "Query history is unavailable",
            "Verified principal",
            "Signed in as",
            "Copy query ID",
            "Copy SQL",
            "ArrowRight",
        ] {
            assert!(shell.contains(expected), "dashboard missing {expected}");
        }
    }
}
