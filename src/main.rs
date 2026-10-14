//! ccdocs: gather the Claude Code conversations of a sidebar group into one HTML file.
//!
//! Output: `<out>/<group>/index.html` plus `<out>/<group>/images/*` for embedded images.

use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const VIEWER_HTML: &str = include_str!("../viewer.html");

const USAGE: &str = "ccdocs --merge-all [--db merged.db]
       ccdocs --serve [--db ccdocs.db] [--port 8765]
       ccdocs --list
       ccdocs --save-all [--db ccdocs.db] [--computer NAME] [--user NAME]
       ccdocs --group NAME [--groups-file groups.json] [--out DIR]
       ccdocs --cwd PATH --name NAME [--out DIR]
options:
  --group NAME         group name, looked up in the groups file (id list per group)
  --groups-file FILE   JSON {\"group\": [\"local_<id>\", ...]}   (default: groups.json)
  --cwd PATH           alternative selector: every desktop session started in PATH
  --name NAME          output folder name when using --cwd
  --out DIR            where the group folder is created (default: .)
  --claude-dir DIR     default: ~/.claude
  --app-dir DIR        Claude desktop data, default: %APPDATA%/Claude";

struct Session {
    local_id: String,
    title: String,
    cwd: String,
    created: i64,
    jsonl: PathBuf,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}\n\n{USAGE}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut a: HashMap<String, String> = HashMap::new();
    let mut it = std::env::args().skip(1);
    let mut list = false;
    let mut save_all = false;
    let mut serve = false;
    let mut merge = false;
    while let Some(k) = it.next() {
        let k = k.strip_prefix("--").ok_or(format!("unexpected argument {k}"))?.to_string();
        match k.as_str() {
            "list" => {
                list = true;
                continue;
            }
            "save-all" => {
                save_all = true;
                continue;
            }
            "merge-all" => {
                merge = true;
                continue;
            }
            "serve" => {
                serve = true;
                continue;
            }
            _ => {}
        }
        a.insert(k.clone(), it.next().ok_or(format!("--{k} needs a value"))?);
    }
    if merge {
        return merge_all(Path::new(a.get("db").map(String::as_str).unwrap_or("merged.db")));
    }
    if serve {
        let db =a.get("db").map(String::as_str).unwrap_or("ccdocs.db");
        let port = a.get("port").map(String::as_str).unwrap_or("8765");
        return serve_viewer(Path::new(db), port);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let claude_dir = a.get("claude-dir").map(PathBuf::from).unwrap_or_else(|| Path::new(&home).join(".claude"));
    let app_dir = a.get("app-dir").map(PathBuf::from).unwrap_or_else(find_app_dir);

    // Index desktop-session metadata (local_<id>.json) and transcripts (<cliSessionId>.jsonl).
    let mut meta: HashMap<String, PathBuf> = HashMap::new();
    walk(&app_dir.join("claude-code-sessions"), &mut |p| {
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            if n.starts_with("local_") && n.ends_with(".json") {
                meta.insert(n.trim_end_matches(".json").to_string(), p.to_path_buf());
            }
        }
    });
    if list {
        return list_groups(a.get("groups-file"), &meta);
    }
    let mut jsonls: HashMap<String, PathBuf> = HashMap::new();
    if let Ok(rd) = fs::read_dir(claude_dir.join("projects")) {
        for d in rd.flatten() {
            if let Ok(fs_) = fs::read_dir(d.path()) {
                for f in fs_.flatten() {
                    let p = f.path();
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        jsonls.insert(p.file_stem().unwrap().to_string_lossy().into(), p);
                    }
                }
            }
        }
    }
    if save_all {
        eprintln!(
            "found {} desktop sessions in {}, {} transcripts in {}",
            meta.len(),
            app_dir.join("claude-code-sessions").display(),
            jsonls.len(),
            claude_dir.join("projects").display()
        );
    }
    if save_all {
        let db = a.get("db").map(String::as_str).unwrap_or("ccdocs.db");
        let computer = a.get("computer").cloned().unwrap_or_else(computer_name);
        let username = a.get("user").cloned().unwrap_or_else(user_name);
        println!("origin: {username}@{computer}");
        return save_all_to_db(Path::new(db), a.get("groups-file"), &meta, &jsonls, &computer, &username);
    }

    let load = |id: &str| -> Option<Session> {
        let v: Value = serde_json::from_str(&fs::read_to_string(meta.get(id)?).ok()?).ok()?;
        let cli = v["cliSessionId"].as_str()?;
        Some(Session {
            local_id: id.to_string(),
            title: v["title"].as_str().unwrap_or("(untitled)").to_string(),
            cwd: v["cwd"].as_str().unwrap_or("").to_string(),
            created: v["createdAt"].as_i64().unwrap_or(0),
            jsonl: jsonls.get(cli)?.clone(),
        })
    };

    let (group, mut sessions) = if let Some(g) = a.get("group") {
        let gf = find_groups_file(a.get("groups-file"));
        let j: Value = match &gf {
            Some(p) => serde_json::from_str(&fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
                .map_err(|e| format!("{}: {e}", p.display()))?,
            None if g == "Ungrouped" => Value::Null,
            None => return Err("groups.json not found (looked in the current folder, next to the exe and up to 4 parents); use --groups-file".into()),
        };
        let mut ss = vec![];
        if g == "Ungrouped" && j.get(g).is_none() {
            // every session that no listed group claims
            let listed: std::collections::HashSet<&str> = j
                .as_object()
                .into_iter()
                .flat_map(|o| o.values())
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            ss.extend(meta.keys().filter(|id| !listed.contains(id.as_str())).filter_map(|id| load(id)));
        } else {
            let ids = j[g].as_array().ok_or(format!("group '{g}' not in groups file"))?;
            for id in ids.iter().filter_map(Value::as_str) {
                match load(id) {
                    Some(s) => ss.push(s),
                    None => eprintln!("skipped {id}: metadata or transcript not found (deleted?)"),
                }
            }
        }
        (g.clone(), ss)
    } else if let Some(cwd) = a.get("cwd") {
        let name = a.get("name").ok_or("--cwd needs --name")?;
        let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        let ss = meta.keys().filter_map(|id| load(id)).filter(|s| norm(&s.cwd) == norm(cwd)).collect();
        (name.clone(), ss)
    } else {
        return Err("need --group or --cwd".into());
    };
    if sessions.is_empty() {
        return Err("no sessions found".into());
    }
    sessions.sort_by_key(|s| s.created);

    let dir = Path::new(a.get("out").map(String::as_str).unwrap_or(".")).join(sanitize(&group));
    let img_dir = dir.join("images");
    fs::create_dir_all(&img_dir).map_err(|e| e.to_string())?;

    let mut toc = String::new();
    let mut body = String::new();
    let mut n_img = 0;
    for (i, s) in sessions.iter().enumerate() {
        let tag = &s.local_id[6..14.min(s.local_id.len())];
        let text = fs::read_to_string(&s.jsonl).map_err(|e| format!("{}: {e}", s.jsonl.display()))?;
        let mut msgs = String::new();
        let mut count = 0;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
                continue;
            }
            let role = match v["type"].as_str() {
                Some(r @ ("user" | "assistant")) => r,
                _ => continue,
            };
            let ts = v["timestamp"].as_str().unwrap_or("");
            let mut inner = String::new();
            match &v["message"]["content"] {
                Value::String(t) => inner.push_str(&md(t)),
                Value::Array(blocks) => {
                    for b in blocks {
                        render_block(b, &img_dir, tag, &mut n_img, &mut inner);
                    }
                }
                _ => {}
            }
            if inner.is_empty() {
                continue;
            }
            count += 1;
            let human = role == "user" && !inner.contains("class=\"result\"");
            let _ = write!(
                msgs,
                "<div class=\"msg {}\"><div class=\"meta\">{} <time>{}</time></div>{inner}</div>\n",
                if human { "user" } else { role },
                if human { "You" } else if role == "user" { "Tool result" } else { "Claude" },
                esc(ts)
            );
        }
        let _ = write!(toc, "<li><a href=\"#s{i}\">{}</a> <small>{} messages</small></li>\n", esc(&s.title), count);
        let _ = write!(
            body,
            "<section id=\"s{i}\"><h2>{}</h2><p class=\"sub\">{} · {} · <a href=\"#top\">top</a></p>{msgs}</section>\n",
            esc(&s.title),
            esc(&s.cwd),
            fmt_ms(s.created)
        );
    }
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{g}</title><style>{CSS}</style></head><body><main id=\"top\"><h1>{g}</h1>\
<p class=\"sub\">{n} conversations</p><ol>{toc}</ol>{body}</main></body></html>",
        g = esc(&group),
        n = sessions.len()
    );
    fs::write(dir.join("index.html"), html).map_err(|e| e.to_string())?;
    if n_img == 0 {
        let _ = fs::remove_dir(&img_dir);
    }
    println!("{} conversations, {} images -> {}", sessions.len(), n_img, dir.join("index.html").display());
    Ok(())
}

fn render_block(b: &Value, img_dir: &Path, tag: &str, n_img: &mut usize, out: &mut String) {
    match b["type"].as_str() {
        Some("text") => out.push_str(&md(b["text"].as_str().unwrap_or(""))),
        Some("thinking") => {
            if let Some(t) = b["thinking"].as_str().filter(|t| !t.is_empty()) {
                let _ = write!(out, "<details><summary>Thinking</summary><div class=\"think\">{}</div></details>", md(t));
            }
        }
        Some("tool_use") => {
            let input = serde_json::to_string_pretty(&b["input"]).unwrap_or_default();
            let _ = write!(
                out,
                "<details class=\"tool\"><summary>Tool: {}</summary><pre>{}</pre></details>",
                esc(b["name"].as_str().unwrap_or("?")),
                esc(&clip(&input, 4000))
            );
        }
        Some("tool_result") => {
            let mut inner = String::new();
            match &b["content"] {
                Value::String(t) => {
                    let _ = write!(inner, "<pre>{}</pre>", esc(&clip(t, 4000)));
                }
                Value::Array(arr) => {
                    for c in arr {
                        if c["type"] == "text" {
                            let _ = write!(inner, "<pre>{}</pre>", esc(&clip(c["text"].as_str().unwrap_or(""), 4000)));
                        } else {
                            render_block(c, img_dir, tag, n_img, &mut inner);
                        }
                    }
                }
                _ => {}
            }
            let err = if b["is_error"] == true { " (error)" } else { "" };
            let _ = write!(out, "<details class=\"result\"><summary>Result{err}</summary>{inner}</details>");
        }
        Some("image") => {
            let src = &b["source"];
            if let (Some(data), Some(mt)) = (src["data"].as_str(), src["media_type"].as_str()) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                    *n_img += 1;
                    let ext = mt.rsplit('/').next().unwrap_or("png").replace("jpeg", "jpg");
                    let name = format!("{tag}-{n_img}.{ext}");
                    if fs::write(img_dir.join(&name), bytes).is_ok() {
                        let _ = write!(out, "<img src=\"images/{name}\" alt=\"image\">");
                    }
                }
            }
        }
        _ => {}
    }
}

/// Minimal markdown: fenced code blocks, inline code, paragraphs.
fn md(s: &str) -> String {
    let mut out = String::new();
    for (i, part) in s.split("```").enumerate() {
        if i % 2 == 1 {
            let code = part.split_once('\n').map_or(part, |(_, rest)| rest);
            let _ = write!(out, "<pre><code>{}</code></pre>", esc(code.trim_end()));
        } else {
            let mut t = String::new();
            for (j, seg) in part.split('`').enumerate() {
                if j % 2 == 1 {
                    let _ = write!(t, "<code>{}</code>", esc(seg));
                } else {
                    t.push_str(&esc(seg));
                }
            }
            if !t.trim().is_empty() {
                let _ = write!(out, "<div class=\"txt\">{}</div>", t.trim());
            }
        }
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [{} more bytes]", &s[..end], s.len() - end)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect()
}

/// Epoch milliseconds -> "YYYY-MM-DD HH:MM" (UTC).
fn fmt_ms(ms: i64) -> String {
    let secs = ms / 1000;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

/// Serve the embedded viewer and the database on http://localhost:<port> and open the browser.
fn serve_viewer(db: &Path, port: &str) -> Result<(), String> {
    if !db.is_file() {
        return Err(format!("{} not found; run --save-all first", db.display()));
    }
    let listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}")).map_err(|e| format!("port {port}: {e}"))?;
    let url = format!("http://localhost:{port}/");
    println!("serving {} at {url} (Ctrl+C to stop)", db.display());
    let _ = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&url).spawn()
    };
    for stream in listener.incoming().flatten() {
        let db = db.to_path_buf();
        std::thread::spawn(move || handle_conn(stream, &db));
    }
    Ok(())
}

fn handle_conn(mut stream: std::net::TcpStream, db: &Path) {
    use std::io::{Read, Write};
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    {
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/");
        let (status, ctype, body): (&str, &str, Vec<u8>) = match path {
            "/" | "/viewer.html" => ("200 OK", "text/html; charset=utf-8", VIEWER_HTML.as_bytes().to_vec()),
            "/ccdocs.db" => match fs::read(db) {
                Ok(b) => ("200 OK", "application/octet-stream", b),
                Err(_) => ("500 Internal Server Error", "text/plain", b"cannot read database".to_vec()),
            },
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
    }
}

fn computer_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .or_else(|| {
            let out = std::process::Command::new("hostname").output().ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn user_name() -> String {
    ["USERNAME", "USER", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "unknown".into())
}

/// Open (creating or upgrading) a ccdocs database.
fn open_db(db: &Path) -> Result<rusqlite::Connection, String> {
    let err = |e: rusqlite::Error| e.to_string();
    let conn = rusqlite::Connection::open(db).map_err(err)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions(
            local_id TEXT PRIMARY KEY, cli_session_id TEXT, title TEXT, cwd TEXT, group_name TEXT,
            created_at INTEGER, last_activity_at INTEGER, archived INTEGER, transcript_path TEXT, saved_at TEXT,
            computer TEXT, username TEXT);
         CREATE TABLE IF NOT EXISTS messages(
            uuid TEXT PRIMARY KEY, local_id TEXT NOT NULL REFERENCES sessions(local_id), parent_uuid TEXT,
            seq INTEGER, role TEXT, timestamp TEXT, is_sidechain INTEGER, model TEXT, text TEXT, content_json TEXT);
         CREATE INDEX IF NOT EXISTS messages_session ON messages(local_id, seq);
         CREATE INDEX IF NOT EXISTS sessions_group ON sessions(group_name);",
    )
    .map_err(err)?;
    // Databases created before origin tracking lack these columns.
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('sessions')")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .map_err(err)?;
    for c in ["computer", "username"] {
        if !cols.iter().any(|x| x == c) {
            conn.execute(&format!("ALTER TABLE sessions ADD COLUMN {c} TEXT"), []).map_err(err)?;
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS sessions_origin ON sessions(computer, username)").map_err(err)?;

    Ok(conn)
}

/// Merge every *.db in the current folder into `out` (created if missing, updated if present).
/// Messages are unioned by uuid; for a conversation present in several sources the most
/// recently saved copy of its session row wins.
fn merge_all(out: &Path) -> Result<(), String> {
    let err = |e: rusqlite::Error| e.to_string();
    let out_abs = fs::canonicalize(out).unwrap_or_else(|_| out.to_path_buf());
    let mut sources: Vec<PathBuf> = fs::read_dir(".")
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("db")))
        .filter(|p| fs::canonicalize(p).map_or(true, |c| c != out_abs))
        .collect();
    sources.sort();
    if sources.is_empty() {
        return Err("no .db files found in the current folder".into());
    }
    let conn = open_db(out)?;
    for src in &sources {
        let path = src.to_string_lossy().into_owned();
        conn.execute("ATTACH DATABASE ?1 AS s", [&path]).map_err(err)?;
        let has = |t: &str| -> bool {
            conn.query_row("SELECT count(*) FROM s.sqlite_master WHERE type='table' AND name=?1", [t], |r| r.get::<_, i64>(0))
                .map_or(false, |n| n > 0)
        };
        if !has("sessions") || !has("messages") {
            eprintln!("skipped {path}: not a ccdocs database");
            conn.execute_batch("DETACH DATABASE s").map_err(err)?;
            continue;
        }
        let scols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('sessions','s')")
            .and_then(|mut q| q.query_map([], |r| r.get(0))?.collect())
            .map_err(err)?;
        let pick = |c: &str| if scols.iter().any(|x| x == c) { c.to_string() } else { "NULL".to_string() };
        let all = "local_id, cli_session_id, title, cwd, group_name, created_at, last_activity_at, archived, transcript_path, saved_at, computer, username";
        let sel = all.split(", ").map(pick).collect::<Vec<_>>().join(", ");
        let upd = all.split(", ").skip(1).map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(", ");
        let before: i64 = conn.query_row("SELECT count(*) FROM main.messages", [], |r| r.get(0)).map_err(err)?;
        conn.execute_batch(&format!(
            "BEGIN;
             INSERT INTO main.sessions({all}) SELECT {sel} FROM s.sessions WHERE true
               ON CONFLICT(local_id) DO UPDATE SET {upd}
               WHERE coalesce(excluded.saved_at,'') >= coalesce(sessions.saved_at,'');
             INSERT OR REPLACE INTO main.messages SELECT * FROM s.messages;
             COMMIT;"
        ))
        .map_err(err)?;
        let after: i64 = conn.query_row("SELECT count(*) FROM main.messages", [], |r| r.get(0)).map_err(err)?;
        println!("{path}: +{} new messages", after - before);
        conn.execute_batch("DETACH DATABASE s").map_err(err)?;
    }
    let (ns, nm): (i64, i64) = conn
        .query_row("SELECT (SELECT count(*) FROM sessions), (SELECT count(*) FROM messages)", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(err)?;
    println!("merged {} databases -> {}: {ns} conversations, {nm} messages", sources.len(), out.display());
    Ok(())
}

/// Create or update a SQLite database holding every conversation (re-running upserts by uuid).
fn save_all_to_db(
    db: &Path,
    groups_file: Option<&String>,
    meta: &HashMap<String, PathBuf>,
    jsonls: &HashMap<String, PathBuf>,
    computer: &str,
    username: &str,
) -> Result<(), String> {
    let err = |e: rusqlite::Error| e.to_string();
    let mut group_of: HashMap<String, String> = HashMap::new();
    if let Some(p) = find_groups_file(groups_file) {
        let j: Value = serde_json::from_str(&fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        for (name, ids) in j.as_object().ok_or("groups file must be a JSON object")? {
            for id in ids.as_array().into_iter().flatten().filter_map(Value::as_str) {
                group_of.insert(id.to_string(), name.clone());
            }
        }
    }
    let mut conn = open_db(db)?;

    let (mut n_sessions, mut n_msgs) = (0, 0);
    for (id, mpath) in meta {
        let Ok(v) = serde_json::from_str::<Value>(&fs::read_to_string(mpath).unwrap_or_default()) else { continue };
        let Some(cli) = v["cliSessionId"].as_str() else { continue };
        let Some(jpath) = jsonls.get(cli) else {
            eprintln!("skipped {id}: transcript not found");
            continue;
        };
        let text = fs::read_to_string(jpath).map_err(|e| format!("{}: {e}", jpath.display()))?;
        let tx = conn.transaction().map_err(err)?;
        tx.execute(
            "INSERT OR REPLACE INTO sessions(local_id, cli_session_id, title, cwd, group_name, created_at,
                last_activity_at, archived, transcript_path, saved_at, computer, username)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,datetime('now'),?10,?11)",
            rusqlite::params![
                id,
                cli,
                v["title"].as_str(),
                v["cwd"].as_str(),
                group_of.get(id).map_or("Ungrouped", String::as_str),
                v["createdAt"].as_i64(),
                v["lastActivityAt"].as_i64(),
                v["isArchived"].as_bool().unwrap_or(false),
                jpath.to_string_lossy(),
                computer,
                username,
            ],
        )
        .map_err(err)?;
        {
            let mut ins = tx
                .prepare_cached("INSERT OR REPLACE INTO messages VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")
                .map_err(err)?;
            for (seq, line) in text.lines().enumerate() {
                let Ok(m) = serde_json::from_str::<Value>(line) else { continue };
                let (Some(role @ ("user" | "assistant")), Some(uuid)) = (m["type"].as_str(), m["uuid"].as_str()) else {
                    continue;
                };
                let content = &m["message"]["content"];
                let plain = match content {
                    Value::String(s) => s.clone(),
                    Value::Array(bs) => bs
                        .iter()
                        .filter(|b| b["type"] == "text")
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                ins.execute(rusqlite::params![
                    uuid,
                    id,
                    m["parentUuid"].as_str(),
                    seq as i64,
                    role,
                    m["timestamp"].as_str(),
                    m["isSidechain"].as_bool().unwrap_or(false),
                    m["message"]["model"].as_str(),
                    plain,
                    content.to_string(),
                ])
                .map_err(err)?;
                n_msgs += 1;
            }
        }
        tx.commit().map_err(err)?;
        n_sessions += 1;
    }
    println!("saved {n_sessions} conversations, {n_msgs} messages -> {}", db.display());
    Ok(())
}

/// Locate the Claude desktop data folder. The Store/MSIX build of the app keeps it under
/// %LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude; only programs started from inside
/// the app see it at %APPDATA%\Claude, so a normal shell has to look in the package folder.
fn find_app_dir() -> PathBuf {
    let has_sessions = |d: &Path| fs::read_dir(d.join("claude-code-sessions")).is_ok_and(|mut r| r.next().is_some());
    let home = std::env::var("HOME").unwrap_or_default();
    let unix_default = if cfg!(target_os = "macos") {
        Path::new(&home).join("Library/Application Support/Claude")
    } else {
        Path::new(&home).join(".config/Claude")
    };
    if !cfg!(windows) {
        return unix_default;
    }
    let default = Path::new(&std::env::var("APPDATA").unwrap_or_default()).join("Claude");
    if has_sessions(&default) {
        return default;
    }
    let packages = Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Packages");
    if let Ok(rd) = fs::read_dir(packages) {
        let mut cands: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("Claude_") && !n.contains(' ') // skip hand-made "... - Copy" folders
            })
            .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude"))
            .filter(|d| has_sessions(d))
            .collect();
        cands.sort();
        if let Some(d) = cands.into_iter().next() {
            return d;
        }
    }
    default
}

/// Print every group in the groups file with its session count, plus the implicit "Ungrouped".
fn list_groups(explicit: Option<&String>, meta: &HashMap<String, PathBuf>) -> Result<(), String> {
    let mut listed = std::collections::HashSet::new();
    match find_groups_file(explicit) {
        Some(p) => {
            let j: Value = serde_json::from_str(&fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)
                .map_err(|e| format!("{}: {e}", p.display()))?;
            for (name, ids) in j.as_object().ok_or("groups file must be a JSON object")? {
                let ids: Vec<&str> = ids.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let present = ids.iter().filter(|id| meta.contains_key(**id)).count();
                println!("{name}  ({present} of {} sessions found)", ids.len());
                listed.extend(ids.into_iter().map(String::from));
            }
        }
        None => eprintln!("(no groups.json found; only Ungrouped is available)"),
    }
    let ungrouped = meta.keys().filter(|id| !listed.contains(*id)).count();
    println!("Ungrouped  ({ungrouped} sessions, every session not in a group above)");
    Ok(())
}

/// Explicit path, else groups.json in the current folder, next to the exe, or in a parent of either.
fn find_groups_file(explicit: Option<&String>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))];
    starts
        .into_iter()
        .flatten()
        .flat_map(|d| d.ancestors().take(5).map(|a| a.join("groups.json")).collect::<Vec<_>>())
        .find(|p| p.is_file())
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, f);
            } else {
                f(&p);
            }
        }
    }
}

const CSS: &str = ":root{--bg:#fff;--fg:#1f2328;--mut:#6b7280;--card:#f6f8fa;--user:#e8f0fe;--bd:#d0d7de}\
@media(prefers-color-scheme:dark){:root{--bg:#14161a;--fg:#e6e8eb;--mut:#9aa3ad;--card:#1d2026;--user:#1f2d45;--bd:#333943}}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}\
main{max-width:860px;margin:0 auto;padding:24px 16px}h2{margin-top:2.5em;border-bottom:1px solid var(--bd);padding-bottom:.3em}\
.sub,small,time,.meta{color:var(--mut);font-size:.85em}.msg{margin:12px 0;padding:10px 14px;border-radius:8px;background:var(--card)}\
.msg.user{background:var(--user)}.txt{white-space:pre-wrap;overflow-wrap:anywhere}\
pre{overflow:auto;background:var(--bg);border:1px solid var(--bd);border-radius:6px;padding:8px;font-size:.85em}\
code{font-family:ui-monospace,Consolas,monospace}details{margin:6px 0}summary{cursor:pointer;color:var(--mut)}\
.think{white-space:pre-wrap;color:var(--mut)}img{max-width:100%;border-radius:6px}a{color:#0969da}";
