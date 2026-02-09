//! Watcher configuration — file extensions that trigger live reload.
//!
//! By default, Hotplate only watches UI-related file extensions:
//!   html, htm, css, scss, sass, less,
//!   js, jsx, ts, tsx, mjs, cjs,
//!   json, svg, png, jpg, jpeg, gif, webp, ico,
//!   woff, woff2, ttf, eot,
//!   xml, md, tx
//!
//! Users can override this via:
//!   - CLI: `--watch-ext html --watch-ext css --watch-ext js`
//!   - VS Code settings: `"hotplate.watchExtensions": ["html", "css", "js"]`
//!   - Use `"*"` to watch ALL file extensions (disable filtering)
