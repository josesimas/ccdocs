//! ccdocs: gather the Claude Code conversations of a sidebar group into one HTML file.
//!
//! Output: `<out>/<group>/index.html` plus `<out>/<group>/images/*` for embedded images.

use base64::Engine;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE: &str = "ccdocs --group NAME [--groups-file groups.json] [--out DIR]
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
    while let Some(k) = it.next() {
        let k = k.strip_prefix("--").ok_or(format!("unexpected argument {k}"))?.to_string();
        a.insert(k.clone(), it.next().ok_or(format!("--{k} needs a value"))?);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let claude_dir = a.get("claude-dir").map(PathBuf::from).unwrap_or_else(|| Path::new(&home).join(".claude"));
    let app_dir = a.get("app-dir").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(&std::env::var("APPDATA").unwrap_or_default()).join("Claude")
    });

    // Index desktop-session metadata (local_<id>.json) and transcripts (<cliSessionId>.jsonl).
    let mut meta: HashMap<String, PathBuf> = HashMap::new();
    walk(&app_dir.join("claude-code-sessions"), &mut |p| {
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            if n.starts_with("local_") && n.ends_with(".json") {
                meta.insert(n.trim_end_matches(".json").to_string(), p.to_path_buf());
            }
        }
    });
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
