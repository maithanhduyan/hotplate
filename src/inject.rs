//! HTML injection middleware — inserts live-reload WebSocket script before </body>.

use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use http_body_util::BodyExt;

/// Live-reload client script with CSS hot-swap + browser event collection.
/// Connects to ws(s)://<host>/__lr (bidirectional).
///
/// Server → Browser:
///   - "reload"      → full page reload
///   - "css:<path>"  → hot-swap only that stylesheet
///
/// Browser → Server (JSON):
///   - {kind:"connect",url,ua,vw,vh}   — client identity on connect
///   - {kind:"js_error",msg,src,line,col,stack}  — runtime JS errors
///   - {kind:"console",level,msg}       — console.warn/error
///   - {kind:"net_error",url,method,status,error} — failed fetch requests
///
/// Auto-reconnects after 1s on disconnect.
const RELOAD_SCRIPT: &str = r#"<script>
(()=>{
  const p=location.protocol==='https:'?'wss:':'ws:';
  let t,ws;
  function send(obj){try{if(ws&&ws.readyState===1)ws.send(JSON.stringify(obj))}catch(_){}}
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
  // Intercept console.warn and console.error
  const _warn=console.warn,_err=console.error;
  console.warn=(...a)=>{send({kind:'console',level:'warn',msg:a.join(' ')});_warn.apply(console,a)};
  console.error=(...a)=>{send({kind:'console',level:'error',msg:a.join(' ')});_err.apply(console,a)};
  // Capture unhandled JS errors
  window.onerror=(msg,src,line,col,err)=>{
    send({kind:'js_error',msg:String(msg),src:src||'',line:line||0,col:col||0,stack:err&&err.stack||''});
  };
  window.onunhandledrejection=(e)=>{
    const r=e.reason;
    send({kind:'js_error',msg:String(r),src:'',line:0,col:0,stack:r&&r.stack||''});
  };
  // Wrap fetch to catch network errors
  const _fetch=window.fetch;
  window.fetch=function(){
    const url=arguments[0]&&arguments[0].url||String(arguments[0]);
    const method=(arguments[1]&&arguments[1].method)||'GET';
    return _fetch.apply(this,arguments).then(r=>{
      if(!r.ok)send({kind:'net_error',url:url,method:method,status:r.status,error:r.statusText});
      return r;
    }).catch(e=>{
      send({kind:'net_error',url:url,method:method,status:0,error:e.message});
      throw e;
    });
  };
  function connect(){
    ws=new WebSocket(`${p}//${location.host}/__lr`);
    ws.onopen=()=>{
      send({kind:'connect',url:location.href,ua:navigator.userAgent,vw:innerWidth,vh:innerHeight});
    };
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
