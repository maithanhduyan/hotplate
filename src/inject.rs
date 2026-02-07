//! HTML injection middleware — inserts live-reload WebSocket script before </body>.

use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use http_body_util::BodyExt;

/// Live-reload client script with CSS hot-swap support.
/// Connects to ws(s)://<host>/__lr.
///   - On "reload" message → full page reload.
///   - On "css:<path>" message → hot-swap only that stylesheet (no page reload).
/// Auto-reconnects after 1s on disconnect.
const RELOAD_SCRIPT: &str = r#"<script>
(()=>{
  const p=location.protocol==='https:'?'wss:':'ws:';
  let t;
  function reloadCSS(path){
    const links=document.querySelectorAll('link[rel="stylesheet"]');
    let found=false;
    links.forEach(link=>{
      const href=link.getAttribute('href');
      if(!href)return;
      const clean=href.split('?')[0];
      if(clean===path||clean==='/'+path||clean.endsWith('/'+path)){
        link.href=clean+'?_lr='+Date.now();
        found=true;
      }
    });
    if(!found)location.reload();
  }
  function connect(){
    const ws=new WebSocket(`${p}//${location.host}/__lr`);
    ws.onmessage=e=>{
      const d=e.data;
      if(d==='reload')location.reload();
      else if(d.startsWith('css:'))reloadCSS(d.slice(4));
    };
    ws.onclose=()=>{clearTimeout(t);t=setTimeout(connect,1000)};
    ws.onerror=()=>ws.close();
  }
  connect();
})();
</script>"#;

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

    // Inject before </body>, or </html>, or at the end
    let injected = if let Some(pos) = html.rfind("</body>") {
        format!("{}{}\n{}", &html[..pos], RELOAD_SCRIPT, &html[pos..])
    } else if let Some(pos) = html.rfind("</html>") {
        format!("{}{}\n{}", &html[..pos], RELOAD_SCRIPT, &html[pos..])
    } else {
        format!("{}\n{}", html, RELOAD_SCRIPT)
    };

    // Remove old Content-Length (body size changed)
    parts.headers.remove(header::CONTENT_LENGTH);

    Response::from_parts(parts, Body::from(injected))
}
