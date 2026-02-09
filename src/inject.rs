//! HTML injection middleware — inserts live-reload WebSocket script before </body>.

use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use http_body_util::BodyExt;

/// Live-reload + browser agent script, loaded from `src/livereload.js`.
/// Using `include_str!` embeds the JS at compile time — zero runtime cost,
/// and the JS file gets proper syntax highlighting & lint in the IDE.
const RELOAD_JS: &str = include_str!("livereload.js");

/// Axum middleware: if the response is HTML, inject the reload script.
pub async fn inject_livereload(req: Request<Body>, next: Next) -> Response<Body> {
    let resp = next.run(req).await;

    // Only process text/html responses
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if !is_html {
        return resp;
    }

    // Buffer the body
    let (mut parts, body) = resp.into_parts();
    let collected = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    let html = String::from_utf8_lossy(&collected);

    // Build <script>...</script> from the external JS file
    let reload_script = format!("<script>\n{}\n</script>", RELOAD_JS);

    // Inject before </body>, or </html>, or at the end
    let injected = if let Some(pos) = html.rfind("</body>") {
        format!("{}{}\n{}", &html[..pos], reload_script, &html[pos..])
    } else if let Some(pos) = html.rfind("</html>") {
        format!("{}{}\n{}", &html[..pos], reload_script, &html[pos..])
    } else {
        format!("{}\n{}", html, reload_script)
    };

    // Remove old Content-Length (body size changed)
    parts.headers.remove(header::CONTENT_LENGTH);

    Response::from_parts(parts, Body::from(injected))
}
