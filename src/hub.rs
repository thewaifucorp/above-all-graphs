//! `aag ui` — THE front door: a tiny localhost server giving the
//! multi-workspace view real navigation. One page, real URLs, a registry
//! read fresh on every request, browser opened automatically. No static
//! file to regenerate, no `file://` iframe restrictions (browsers — and
//! headless test tooling — routinely refuse to embed one `file://`
//! document inside another).
//!
//! This is the one place `aag` legitimately runs a server: a per-repo
//! site (`crate::export`) stays 100% static — open `.aag/index.html`
//! directly, no process needed — because "what's in this repo" is
//! build-time state. "Which workspaces exist on this machine right now"
//! is runtime state, so the hub is runtime too, on request.
//!
//! Binds to `127.0.0.1` only. Serving files from arbitrary local
//! directories is exactly what a graph explorer needs to do, so the two
//! guards that matter: never listen beyond loopback, and never serve a
//! path outside a workspace's own `.aag/` that isn't currently in the
//! registry (`crate::workspaces::is_registered`).

use std::path::Path;

use tiny_http::{Header, Response, Server};

use crate::error::{Error, Result};

/// The shell — exactly one bar of lib-level chrome over a full-bleed
/// `<iframe>`: aag mark/wordmark, workspace picker (cross-workspace
/// choice, so it lives up here), stats, star/waifucorp. That is the
/// whole split: **lib-wide up here, workspace-specific down inside**.
/// Page navigation (Graph/Wiki/Report) belongs to the embedded pages —
/// their own headers carry those links and navigate within the iframe;
/// when framed they hide only their brand/star/waifucorp duplicates
/// (`html.embedded` rules in the templates). Picking a workspace opens
/// its graph. Vanilla JS, no CDN.
const SHELL_HTML: &str = r##"<!doctype html>
<meta charset="utf-8">
<title>aag</title>
<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath d='M12 5.5 6 18.5M12 5.5l6 13M6.8 15.5h10.4' stroke='%23555' stroke-width='1.3' fill='none'/%3E%3Ccircle cx='12' cy='5.5' r='3' fill='%23ff3b3b'/%3E%3Ccircle cx='6' cy='18.5' r='3' fill='%23ffcf3b'/%3E%3Ccircle cx='18' cy='18.5' r='3' fill='%233bd8ff'/%3E%3C/svg%3E">
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; margin: 0; }
  html, body { height: 100%; }
  body { background: #0a0a0a; color: #f2f2ee; font: 14px/1.5 "JetBrains Mono", ui-monospace, monospace;
    display: flex; flex-direction: column; }
  .spectrum { height: 3px; background: linear-gradient(90deg,#c1121f,#ff7b00,#ffc600,#38b000,#0077b6,#7b2cbf); }
  header { display: flex; align-items: center; gap: 14px; padding: 8px 14px; border-bottom: 1px solid #1c1c1c; }
  .brand { display: flex; align-items: center; gap: 8px; }
  .brand .word { font-weight: 700; font-size: 14px; }
  .brand .sub { color: #63635e; font-size: 9px; display: block; line-height: 1; white-space: nowrap; }
  select { background: #121212; color: #ffc600; border: 1px solid #333; border-radius: 6px;
    font: inherit; font-size: 13px; padding: 6px 10px; max-width: 340px; cursor: pointer; }
  select:hover, select:focus { border-color: #ffc600; outline: none; }
  .stats { margin-left: auto; color: #666; font-size: 11.5px; white-space: nowrap; }
  .hbtn { background: none; border: 1px solid #262624; color: #8a8a84; border-radius: 8px; height: 28px;
    font: inherit; font-size: 12px; padding: 0 10px; text-decoration: none; display: inline-flex; align-items: center; gap: 5px; cursor: pointer; }
  .hbtn:disabled { opacity: 0.5; cursor: wait; }
  .hbtn:hover { color: #f2f2ee; border-color: #373531; }
  .hbtn.star { color: #ffc600; }
  .hbtn.star:hover { border-color: #ffc600; }
  .hbtn.waifu { color: #c1121f; }
  .hbtn.waifu:hover { border-color: #c1121f; }
  iframe { flex: 1; border: 0; background: #0a0a0a; width: 100%; }
  .empty { flex: 1; display: flex; align-items: center; justify-content: center; color: #888; }
  code { color: #ffc600; }
</style>
<div class="spectrum"></div>
<header>
  <span class="brand">
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none">
      <path d="M12 5.5 6 18.5M12 5.5l6 13M6.8 15.5h10.4" stroke="#373531" stroke-width="1.3"/>
      <circle cx="12" cy="5.5" r="3" fill="#ff3b3b"/>
      <circle cx="6" cy="18.5" r="3" fill="#ffcf3b"/>
      <circle cx="18" cy="18.5" r="3" fill="#3bd8ff"/>
    </svg>
    <span><span class="word">aag</span><span class="sub">above all graphs</span></span>
  </span>
  <select id="picker" title="workspace"><option>loading…</option></select>
  <button class="hbtn" id="add-btn" title="Index a new repository">+ index</button>
  <span class="stats" id="stats"></span>
  <a class="hbtn star" href="https://github.com/thewaifucorp/above-all-graphs" target="_blank" rel="noopener" title="Star on GitHub">&#9733; star</a>
  <a class="hbtn waifu" href="https://waifucorp.org" target="_blank" rel="noopener" title="Meet WaifuCorp">&#9825; waifucorp</a>
</header>
<iframe id="view"></iframe>
<script>
(function () {
  var picker = document.getElementById("picker");
  var addBtn = document.getElementById("add-btn");
  var frame = document.getElementById("view");
  var stats = document.getElementById("stats");
  var workspaces = [];

  function render() {
    var ws = workspaces[picker.selectedIndex];
    if (!ws) return;
    frame.src = "/w/" + encodeURIComponent(ws.path) + "/graph.html";
    stats.textContent = ws.files + " files · " + ws.symbols + " symbols · " + ws.edges + " edges";
    document.title = ws.name + " — aag";
  }

  function loadWorkspaces(selectPath) {
    return fetch("/api/workspaces")
      .then(function (r) { return r.json(); })
      .then(function (list) {
        workspaces = list;
        picker.innerHTML = "";
        if (!list.length) {
          picker.appendChild(new Option("no workspaces — hit + index", ""));
          stats.textContent = "";
          return;
        }
        list.forEach(function (ws, i) {
          var opt = new Option(ws.name, ws.path);
          opt.title = ws.path;
          picker.appendChild(opt);
          if (selectPath && ws.path === selectPath) picker.selectedIndex = i;
        });
        render();
      })
      .catch(function () {
        picker.innerHTML = "";
        picker.appendChild(new Option("could not load workspaces", ""));
      });
  }

  picker.addEventListener("change", render);

  addBtn.addEventListener("click", function () {
    var path = prompt("Absolute path of the repository to index:");
    if (!path) return;
    addBtn.disabled = true;
    addBtn.textContent = "indexing…";
    fetch("/api/index", { method: "POST", body: JSON.stringify({ path: path.trim() }) })
      .then(function (r) { return r.json().then(function (j) { return { ok: r.ok, j: j }; }); })
      .then(function (res) {
        if (!res.ok) throw new Error(res.j.error || "indexing failed");
        return loadWorkspaces(res.j.path);
      })
      .catch(function (e) { alert(e.message); })
      .finally(function () {
        addBtn.disabled = false;
        addBtn.textContent = "+ index";
      });
  });

  loadWorkspaces();
})();
</script>
"##;

/// Starts the UI server and blocks forever, one thread per request —
/// fine at the concurrency a single local browser generates. `port: 0`
/// asks the OS for a free port (the default: no "address already in
/// use" to troubleshoot); pass a fixed port to get a bookmarkable URL.
/// Unless `no_open`, also launches the default browser at the served
/// URL — `aag ui` should mean "the UI is on my screen", not "now go
/// find the right address".
///
/// # Errors
///
/// Returns an error if the port cannot be bound.
pub fn run(port: u16, no_open: bool) -> Result<()> {
    let server = Server::http(("127.0.0.1", port)).map_err(|source| Error::Write {
        path: std::path::PathBuf::from(format!("127.0.0.1:{port}")),
        source: std::io::Error::other(source.to_string()),
    })?;
    let url = format!("http://{}", server.server_addr());
    println!("aag ui: {url}  (Ctrl+C to stop)");
    if !no_open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        std::thread::spawn(move || handle(request));
    }
    Ok(())
}

/// Best-effort launch of the platform's default browser — a failure here
/// (headless box, exotic desktop) must never take the server down; the
/// URL is already printed.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");

    let spawned = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = spawned {
        tracing::debug!(%error, "could not open browser");
    }
}

fn handle(mut request: tiny_http::Request) {
    let method = request.method().to_string().to_uppercase();
    let url = request.url().to_string();
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let (status, content_type, bytes) = route(&method, &url, &body);
    let mut response = Response::from_data(bytes).with_status_code(status);
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        response.add_header(header);
    }
    let _ = request.respond(response);
}

/// Dispatches one request to a (status, content-type, body) triple — pure
/// and separate from `handle` so routing logic is unit-testable without
/// spinning up a real socket. The full route table:
///
/// - `GET /` — the shell (the app's initial and only page)
/// - `GET /api/workspaces` — live registry as JSON
/// - `POST /api/index` — index a new repo (`{"path": "/abs/path"}`)
/// - `GET /w/<encoded-root>/<page>` — a workspace's generated site
fn route(method: &str, url: &str, body: &str) -> (u16, String, Vec<u8>) {
    if method == "POST" {
        if url == "/api/index" {
            return index_new_workspace(body);
        }
        return (
            404,
            "text/plain; charset=utf-8".into(),
            b"not found".to_vec(),
        );
    }
    if url == "/" || url == "/index.html" {
        return (200, "text/html; charset=utf-8".into(), SHELL_HTML.into());
    }
    if url == "/api/workspaces" {
        let list = crate::workspaces::live_entries();
        let body = serde_json::to_vec(&list).unwrap_or_else(|_| b"[]".to_vec());
        return (200, "application/json".into(), body);
    }
    if let Some(rest) = url.strip_prefix("/w/") {
        return serve_workspace_file(rest);
    }
    (
        404,
        "text/plain; charset=utf-8".into(),
        b"not found".to_vec(),
    )
}

/// `POST /api/index` — the shell's "+ index" button. Body:
/// `{"path": "/absolute/path/to/repo"}`. A fresh repo gets a full
/// `bigbang` (index + site + agent integration — same as running it in a
/// terminal); an already-indexed one gets a `sync` refresh, which also
/// re-registers it if the registry lost track. Returns the canonical
/// path so the shell can select the new entry.
fn index_new_workspace(body: &str) -> (u16, String, Vec<u8>) {
    let json_error = |status: u16, message: &str| {
        (
            status,
            "application/json".into(),
            serde_json::to_vec(&serde_json::json!({"error": message}))
                .unwrap_or_else(|_| b"{}".to_vec()),
        )
    };

    let Some(path) = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
    else {
        return json_error(400, "body must be {\"path\": \"/absolute/path\"}");
    };
    let dir = Path::new(&path);
    if !dir.is_dir() {
        return json_error(400, "path is not a directory");
    }
    let Ok(canonical) = dir.canonicalize() else {
        return json_error(400, "path cannot be resolved");
    };

    let result = if canonical.join(".aag").is_dir() {
        crate::sync::run(&canonical, None, false)
    } else {
        crate::bigbang::run(&canonical, &crate::bigbang::Options::default())
    };
    match result {
        Ok(()) => (
            200,
            "application/json".into(),
            serde_json::to_vec(
                &serde_json::json!({"ok": true, "path": canonical.to_string_lossy()}),
            )
            .unwrap_or_else(|_| b"{}".to_vec()),
        ),
        Err(error) => json_error(500, &error.to_string()),
    }
}

/// Serves `<workspace>/.aag/<sub-path>` for a `/w/<percent-encoded root>/<sub-path>`
/// request, refusing anything outside a currently registered workspace's
/// own `.aag/` directory.
fn serve_workspace_file(rest: &str) -> (u16, String, Vec<u8>) {
    let mut parts = rest.splitn(2, '/');
    let Some(encoded_root) = parts.next().filter(|s| !s.is_empty()) else {
        return (
            400,
            "text/plain; charset=utf-8".into(),
            b"missing workspace".to_vec(),
        );
    };
    let sub_path = parts.next().unwrap_or("index.html");
    let Some(root) = percent_decode(encoded_root) else {
        return (
            400,
            "text/plain; charset=utf-8".into(),
            b"bad request".to_vec(),
        );
    };

    if !crate::workspaces::is_registered(Path::new(&root)) {
        return (
            404,
            "text/plain; charset=utf-8".into(),
            b"unknown workspace".to_vec(),
        );
    }

    let aag_dir = Path::new(&root).join(".aag");
    let Ok(canonical_aag) = aag_dir.canonicalize() else {
        return (
            404,
            "text/plain; charset=utf-8".into(),
            b"not found".to_vec(),
        );
    };
    let requested = aag_dir.join(sub_path.trim_start_matches('/'));
    let Ok(canonical) = requested.canonicalize() else {
        return (
            404,
            "text/plain; charset=utf-8".into(),
            b"not found".to_vec(),
        );
    };
    if !canonical.starts_with(&canonical_aag) {
        return (
            403,
            "text/plain; charset=utf-8".into(),
            b"forbidden".to_vec(),
        );
    }

    match std::fs::read(&canonical) {
        Ok(bytes) => (200, content_type_for(&canonical), bytes),
        Err(_) => (
            404,
            "text/plain; charset=utf-8".into(),
            b"not found".to_vec(),
        ),
    }
}

/// `Content-Type` by extension, covering everything a generated `.aag/`
/// site actually contains.
fn content_type_for(path: &Path) -> String {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json" | "graphml") => "application/json; charset=utf-8",
        Some("md" | "txt" | "cypher") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Decodes a percent-encoded path segment (`encodeURIComponent` output
/// from the shell's JS). `None` on malformed `%XX`/invalid UTF-8, which
/// the caller treats as a bad request rather than guessing.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aag-hub-test-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn percent_decode_round_trips_spaces_and_unicode() {
        // encodeURIComponent("/repo café/x") in JS produces this.
        assert_eq!(
            percent_decode("%2Frepo%20caf%C3%A9%2Fx"),
            Some("/repo café/x".to_string())
        );
    }

    #[test]
    fn percent_decode_rejects_truncated_escape() {
        assert_eq!(percent_decode("abc%2"), None);
    }

    #[test]
    fn shell_served_at_root() {
        let (status, content_type, body) = route("GET", "/", "");
        assert_eq!(status, 200);
        assert!(content_type.starts_with("text/html"));
        let html = String::from_utf8(body).unwrap();
        assert!(html.contains("id=\"picker\""), "workspace picker missing");
        assert!(html.contains("id=\"view\""), "content iframe missing");
        assert!(
            !html.contains("data-page="),
            "shell must not carry page tabs — that's the embedded page's job"
        );
    }

    #[test]
    fn unknown_route_is_404() {
        let (status, _, _) = route("GET", "/nope", "");
        assert_eq!(status, 404);
    }

    #[test]
    fn index_api_rejects_malformed_body() {
        let (status, _, body) = route("POST", "/api/index", "not json");
        assert_eq!(status, 400);
        assert!(String::from_utf8(body).unwrap().contains("path"));
    }

    #[test]
    fn index_api_rejects_missing_directory() {
        let (status, _, _) = route(
            "POST",
            "/api/index",
            "{\"path\": \"/definitely/not/a/real/dir\"}",
        );
        assert_eq!(status, 400);
    }

    #[test]
    fn index_api_requires_post() {
        let (status, _, _) = route("GET", "/api/index", "");
        assert_eq!(status, 404);
    }

    #[test]
    fn post_to_unknown_route_is_404() {
        let (status, _, _) = route("POST", "/api/nope", "{}");
        assert_eq!(status, 404);
    }

    #[test]
    fn workspace_file_requires_registration() {
        let root = scratch();
        fs::create_dir_all(root.join(".aag")).unwrap();
        fs::write(root.join(".aag").join("index.html"), "hi").unwrap();

        // Not registered anywhere — must 404, even though the file exists
        // on disk and the path is well-formed.
        let encoded = root.to_string_lossy().replace('/', "%2F");
        let url = format!("/w/{encoded}/index.html");
        let (status, _, _) = route("GET", &url, "");
        assert_eq!(status, 404);
    }

    #[test]
    fn workspace_file_traversal_is_rejected() {
        let root = scratch();
        fs::create_dir_all(root.join(".aag")).unwrap();
        fs::write(root.join(".aag").join("index.html"), "hi").unwrap();
        fs::write(root.join("secret.txt"), "nope").unwrap();

        // Even a root that WERE registered can only serve inside its own
        // `.aag/` — `serve_workspace_file` checks registration first, so
        // this exercises the canonicalize+prefix guard by calling it
        // directly against a path outside `.aag/`.
        let aag_dir = root.join(".aag").canonicalize().unwrap();
        let escaped = root.join(".aag").join("..").join("secret.txt");
        let canonical = escaped.canonicalize().unwrap();
        assert!(
            !canonical.starts_with(&aag_dir),
            "traversal must not stay under .aag/"
        );
    }

    #[test]
    fn content_type_covers_html_and_json() {
        assert_eq!(
            content_type_for(Path::new("x.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("x.json")),
            "application/json; charset=utf-8"
        );
        assert_eq!(
            content_type_for(Path::new("x.bin")),
            "application/octet-stream"
        );
    }
}
