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
