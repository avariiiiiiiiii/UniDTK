//! The project book: `docs/SUMMARY.md` and the pages it lists, rendered to HTML
//! by `udtk doc` and checked by `udtk doc --check`.

use pulldown_cmark::{html, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::path::{Path, PathBuf};

const REPOSITORY: &str = "https://github.com/avariiiiiiiiii/UniDTK/blob/main";
/// One file of the reference must stay readable in a single tool call.
const REFERENCE_LINE_LIMIT: usize = 350;
const CODE_DIRS: [&str; 5] = ["core", "modules", "opt/cli", "toolchain", ".cargo"];
const CODE_EXTENSIONS: [&str; 7] = ["rs", "js", "css", "html", "toml", "nsh", "json"];
const SKIPPED_DIRS: [&str; 4] = ["target", "dist", "gen", "node_modules"];
/// Code copied from another project keeps its own comments, which cite that
/// project's documents (see `core/rune/UPSTREAM.md`).
const IMPORTED_DIRS: [&str; 1] = ["core/rune/"];
/// The checker itself: its tests spell out the very mistakes it looks for.
const SELF: &str = "opt/cli/book.rs";

pub struct Entry {
    pub title: String,
    /// Repository-relative, forward slashes.
    pub path: String,
    pub part: Option<String>,
}

/// The mdBook subset: `# Part` headings and one `[title](path)` per line, with
/// paths relative to `docs/`. The first `# Summary` heading is the file's title.
pub fn parse_summary(text: &str) -> Vec<Entry> {
    let mut part = None;
    let mut entries = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("# ") {
            part = (heading != "Summary").then(|| heading.to_string());
            continue;
        }
        let line = line.strip_prefix("- ").unwrap_or(line);
        let parsed = line
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("]("))
            .and_then(|(title, rest)| Some((title, rest.strip_suffix(')')?)));
        if let Some((title, path)) = parsed {
            entries.push(Entry {
                title: title.to_string(),
                path: normalize("docs", path),
                part: part.clone(),
            });
        }
    }
    entries
}

/// GitHub's heading anchor, so that a link works on GitHub and in the book alike.
pub fn slug(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn unique(seen: &mut Vec<String>, base: String) -> String {
    let mut candidate = base.clone();
    let mut n = 0;
    while seen.contains(&candidate) {
        n += 1;
        candidate = format!("{base}-{n}");
    }
    seen.push(candidate.clone());
    candidate
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// `r[storage.no-raw-db]` alone on a line defines a rule; outside `docs/reference`
/// the same form cites one.
fn rule_definition(line: &str) -> Option<&str> {
    let id = line.trim_end().strip_prefix("r[")?.strip_suffix(']')?;
    is_rule_id(id).then_some(id)
}

fn is_rule_id(id: &str) -> bool {
    id.contains('.')
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
}

/// Rule ids cited anywhere in `text`, definitions included.
fn rule_citations(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    let mut previous = ' ';
    while let Some(at) = rest.find("r[") {
        if at > 0 {
            previous = rest[..at].chars().last().unwrap_or(' ');
        }
        let tail = &rest[at + 2..];
        if let Some((id, _)) = tail.split_once(']') {
            if is_rule_id(id) && !previous.is_alphanumeric() {
                found.push(id.to_string());
            }
        }
        rest = tail;
        previous = '[';
    }
    found
}

/// Runs `visit` on every line outside a fenced code block.
fn prose_lines(text: &str) -> Vec<&str> {
    let mut fenced = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            lines.push(line);
        }
    }
    lines
}

fn rule_definitions(text: &str) -> Vec<String> {
    prose_lines(text).into_iter().filter_map(rule_definition).map(str::to_string).collect()
}

pub fn anchors(text: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut heading: Option<String> = None;
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => {
                if let Some(title) = heading.take() {
                    unique(&mut seen, slug(&title));
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(title) = heading.as_mut() {
                    title.push_str(&t);
                }
            }
            _ => {}
        }
    }
    seen.extend(rule_definitions(text).into_iter().map(|id| format!("r-{id}")));
    seen
}

/// Joins `relative` onto the repository-relative `dir` and resolves the `..`s.
pub fn normalize(dir: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The path from the directory `from_dir` to the repository path `target`.
pub fn relative(from_dir: &str, target: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = target.split('/').collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - shared];
    parts.extend_from_slice(&to[shared..]);
    parts.join("/")
}

fn dir_of(page: &str) -> &str {
    page.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The rendered counterpart of a markdown page.
fn html_path(page: &str) -> String {
    if page == "README.md" {
        "index.html".to_string()
    } else {
        format!("{}.html", page.strip_suffix(".md").unwrap_or(page))
    }
}

/// A link in `page`, as it must read in a file that sits in `view_dir`. Pages of
/// the book stay local; anything else (source files, unshipped history) goes to
/// the repository, because the installed book has neither.
pub fn rewrite_link(dest: &str, page: &str, view_dir: &str, rendered: &[String]) -> String {
    if dest.contains("://") || dest.starts_with('#') || dest.starts_with("mailto:") {
        return dest.to_string();
    }
    let (path, fragment) = dest.split_once('#').map_or((dest, ""), |(p, f)| (p, f));
    let target = normalize(dir_of(page), path);
    let suffix = if fragment.is_empty() { String::new() } else { format!("#{fragment}") };
    if rendered.iter().any(|p| *p == target) {
        let local = if target.ends_with(".md") { html_path(&target) } else { target };
        format!("{}{suffix}", relative(view_dir, &local))
    } else {
        format!("{REPOSITORY}/{target}{suffix}")
    }
}

/// Turns each rule definition into an anchor the page can link to.
fn expand_rules(text: &str) -> String {
    let mut fenced = false;
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        match rule_definition(line).filter(|_| !fenced) {
            Some(id) => out.push_str(&format!(
                "<a class=\"rule\" id=\"r-{id}\" href=\"#r-{id}\">[{id}]</a>\n\n"
            )),
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

pub fn render_body(text: &str, page: &str, view_dir: &str, rendered: &[String]) -> String {
    let text = expand_rules(text);
    let mut events: Vec<Event> = Vec::new();
    let mut seen = Vec::new();
    let mut heading: Option<(HeadingLevel, Vec<Event>)> = None;
    let mut metadata = false;
    for event in Parser::new_ext(&text, options()) {
        match event {
            Event::Start(Tag::MetadataBlock(_)) => metadata = true,
            Event::End(TagEnd::MetadataBlock(_)) => metadata = false,
            _ if metadata => {}
            Event::Start(Tag::Heading { level, .. }) => heading = Some((level, Vec::new())),
            Event::End(TagEnd::Heading(_)) => {
                let Some((level, inner)) = heading.take() else { continue };
                let title: String = inner
                    .iter()
                    .filter_map(|e| match e {
                        Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                        _ => None,
                    })
                    .collect();
                let id = unique(&mut seen, slug(&title));
                events.push(Event::Start(Tag::Heading {
                    level,
                    id: Some(CowStr::from(id)),
                    classes: Vec::new(),
                    attrs: Vec::new(),
                }));
                events.extend(inner);
                events.push(Event::End(TagEnd::Heading(level)));
            }
            other => {
                let other = match other {
                    Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                        let dest_url = CowStr::from(rewrite_link(&dest_url, page, view_dir, rendered));
                        Event::Start(Tag::Link { link_type, dest_url, title, id })
                    }
                    other => other,
                };
                match heading.as_mut() {
                    Some((_, inner)) => inner.push(other),
                    None => events.push(other),
                }
            }
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    body
}

const STYLE: &str = "\
:root{--bg:#fff;--fg:#1b1c20;--dim:#62656e;--line:#dfe1e6;--code:#f4f4f5;--link:#1a56c4}\
@media(prefers-color-scheme:dark){:root{--bg:#17181c;--fg:#e6e6e6;--dim:#9a9da6;--line:#2e3037;--code:#23252b;--link:#8ab4f8}}\
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.6 system-ui,sans-serif}\
a{color:var(--link)}nav{position:fixed;inset:0 auto 0 0;width:19rem;overflow-y:auto;padding:1.25rem;border-right:1px solid var(--line);font-size:14px}\
nav h2{margin:1.25rem 0 .25rem;font-size:12px;color:var(--dim);text-transform:uppercase;letter-spacing:.05em}\
nav ol{list-style:none;margin:0;padding:0}nav li{margin:.2rem 0}nav a{text-decoration:none}nav a.here{font-weight:600;color:var(--fg)}\
main{max-width:50rem;margin:0 auto;padding:2rem 1.5rem}body.paged main{margin-left:calc(19rem + max(0px,(100vw - 69rem)/2))}\
pre{overflow-x:auto;padding:.75rem;background:var(--code)}code{font-family:ui-monospace,Consolas,monospace;font-size:.9em}\
table{display:block;overflow-x:auto;border-collapse:collapse}td,th{border:1px solid var(--line);padding:.25rem .5rem}\
blockquote{margin:1rem 0;padding:.25rem 1rem;border-left:3px solid var(--line);color:var(--dim)}\
a.rule{display:block;font:12px ui-monospace,Consolas,monospace;color:var(--dim);text-decoration:none}\
footer{display:flex;justify-content:space-between;gap:1rem;margin-top:3rem;padding-top:1rem;border-top:1px solid var(--line)}\
@media(max-width:60rem){nav{position:static;width:auto;border-right:0;border-bottom:1px solid var(--line)}body.paged main{margin:0 auto}}\
@media print{nav,footer{display:none}body.paged main{margin:0}}";

fn page_html(title: &str, class: &str, nav: &str, main: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style><body class=\"{class}\">{nav}<main>{main}</main></body></html>"
    )
}

/// `SUMMARY.md` as a sidebar. A listed page that did not ship links to the
/// repository instead of disappearing from the table of contents.
fn sidebar(entries: &[Entry], page: &str, rendered: &[String]) -> String {
    let view = dir_of(page);
    let mut nav = String::from("<nav>");
    let mut part: Option<&str> = None;
    let mut open = false;
    nav.push_str(&format!("<p><a href=\"{}\">Print this book</a></p>", relative(view, "print.html")));
    for entry in entries {
        if entry.part.as_deref() != part || !open {
            if open {
                nav.push_str("</ol>");
            }
            if let Some(name) = entry.part.as_deref() {
                nav.push_str(&format!("<h2>{name}</h2>"));
            }
            nav.push_str("<ol>");
            part = entry.part.as_deref();
            open = true;
        }
        let href = link_to(&entry.path, view, rendered);
        let here = if entry.path == page { " class=\"here\" aria-current=\"page\"" } else { "" };
        nav.push_str(&format!("<li><a{here} href=\"{href}\">{}</a></li>", entry.title));
    }
    if open {
        nav.push_str("</ol>");
    }
    nav.push_str("</nav>");
    nav
}

fn link_to(target: &str, view_dir: &str, rendered: &[String]) -> String {
    rewrite_link(&relative("", target), "", view_dir, rendered)
}

/// Renders the book into `output`, replacing whatever an older run left there.
pub fn build(source: &Path, output: &Path) -> Result<PathBuf, String> {
    let summary = source.join("docs/SUMMARY.md");
    let text = std::fs::read_to_string(&summary).map_err(|e| format!("{}: {e}", summary.display()))?;
    let entries = parse_summary(&text);
    let rendered: Vec<String> = entries.iter().map(|e| e.path.clone()).filter(|p| source.join(p).is_file()).collect();
    match std::fs::remove_dir_all(output) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", output.display())),
        _ => {}
    }

    let mut print = String::new();
    let pages: Vec<&Entry> = entries.iter().filter(|e| rendered.contains(&e.path)).collect();
    for (at, entry) in pages.iter().enumerate() {
        let file = source.join(&entry.path);
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let view = dir_of(&entry.path);
        let mut main = render_body(&text, &entry.path, view, &rendered);
        print.push_str(&render_body(&text, &entry.path, "", &rendered));

        let neighbour = |other: Option<&&Entry>, label: &str| {
            other.map_or(String::from("<span></span>"), |e| {
                format!("<a href=\"{}\">{label} {}</a>", link_to(&e.path, view, &rendered), e.title)
            })
        };
        let previous = at.checked_sub(1).and_then(|i| pages.get(i));
        main.push_str(&format!(
            "<footer>{}{}</footer>",
            neighbour(previous, "←"),
            neighbour(pages.get(at + 1), "→")
        ));

        let target = output.join(html_path(&entry.path));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let html = page_html(&entry.title, "paged", &sidebar(&entries, &entry.path, &rendered), &main);
        std::fs::write(&target, html).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    let print_page = output.join("print.html");
    std::fs::write(&print_page, page_html("UniDTK book", "", "", &print))
        .map_err(|e| format!("{}: {e}", print_page.display()))?;
    Ok(output.join("index.html"))
}

/// History is frozen: it records what was true then, so it is never rewritten
/// to keep its links and commands current.
fn frozen(page: &str) -> bool {
    page.starts_with("docs/history/") || page == "docs/sessions.md"
}

fn markdown_under(dir: &Path, root: &Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            markdown_under(&path, root, found);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Ok(rel) = path.strip_prefix(root) {
                found.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn code_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if !SKIPPED_DIRS.iter().any(|skip| name == *skip) {
                code_files(&path, found);
            }
        } else if path.extension().is_some_and(|e| CODE_EXTENSIONS.iter().any(|x| e == *x)) {
            found.push(path);
        }
    }
}

/// `udtk <word>` in inline code or at the start of a code-block line.
fn commands_in(text: &str) -> Vec<String> {
    fn word(line: &str) -> Option<String> {
        let rest = line.trim_start().strip_prefix("udtk ")?;
        let word: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
        (!word.is_empty()).then_some(word)
    }
    let mut found = Vec::new();
    let mut block = false;
    for event in Parser::new_ext(text, options()) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => block = true,
            Event::End(TagEnd::CodeBlock) => block = false,
            Event::Text(t) if block => found.extend(t.lines().filter_map(word)),
            Event::Code(t) => found.extend(word(&t)),
            _ => {}
        }
    }
    found
}

fn links_in(text: &str) -> Vec<String> {
    Parser::new_ext(text, options())
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. }) => Some(dest_url.to_string()),
            _ => None,
        })
        .collect()
}

fn check_links(root: &Path, page: &str, text: &str, problems: &mut Vec<String>) {
    for dest in links_in(text) {
        if dest.contains("://") || dest.starts_with("mailto:") {
            continue;
        }
        let (path, fragment) = dest.split_once('#').map_or((dest.as_str(), ""), |(p, f)| (p, f));
        let target = if path.is_empty() { page.to_string() } else { normalize(dir_of(page), path) };
        if !root.join(&target).exists() {
            problems.push(format!("{page}: broken link {dest}"));
        } else if !fragment.is_empty() && target.ends_with(".md") {
            let known = std::fs::read_to_string(root.join(&target)).map(|t| anchors(&t)).unwrap_or_default();
            if !known.iter().any(|a| a == fragment) {
                problems.push(format!("{page}: broken anchor {dest}"));
            }
        }
    }
}

fn check_code(root: &Path, rules: &[String], problems: &mut Vec<String>) {
    let mut files = Vec::new();
    for dir in CODE_DIRS {
        code_files(&root.join(dir), &mut files);
    }
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let name = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().replace('\\', "/");
        if name == SELF || IMPORTED_DIRS.iter().any(|dir| name.starts_with(dir)) {
            continue;
        }
        for (at, line) in text.lines().enumerate() {
            let at = at + 1;
            if line.contains('§') && (line.contains(".md") || line.contains("architecture")) {
                problems.push(format!("{name}:{at}: section number, cite a rule: {}", line.trim()));
            }
            let mut rest = line;
            while let Some(start) = rest.find("docs/") {
                let path: String = rest[start..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
                    .collect();
                let path = path.trim_end_matches('.');
                if path.ends_with(".md") && !root.join(path).exists() {
                    problems.push(format!("{name}:{at}: no such document {path}"));
                }
                rest = &rest[start + 5..];
            }
            for id in rule_citations(line) {
                if !rules.contains(&id) {
                    problems.push(format!("{name}:{at}: unknown rule {id}"));
                }
            }
        }
    }
}

/// What can rot in the book. Needs a checkout: the installed copy has no sources.
pub fn check(root: &Path, commands: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let summary = std::fs::read_to_string(root.join("docs/SUMMARY.md")).unwrap_or_default();
    let entries = parse_summary(&summary);
    if entries.is_empty() {
        problems.push("docs/SUMMARY.md: no pages".to_string());
    }
    let mut pages = Vec::new();
    for entry in &entries {
        if root.join(&entry.path).is_file() {
            pages.push(entry.path.clone());
        } else {
            problems.push(format!("docs/SUMMARY.md: missing page {}", entry.path));
        }
    }
    let mut on_disk = Vec::new();
    markdown_under(&root.join("docs"), root, &mut on_disk);
    for page in on_disk {
        if page != "docs/SUMMARY.md" && !pages.contains(&page) {
            problems.push(format!("{page}: not in SUMMARY"));
        }
    }

    let read = |page: &str| std::fs::read_to_string(root.join(page)).unwrap_or_default();
    let mut rules: Vec<String> = Vec::new();
    for page in pages.iter().filter(|p| !frozen(p)) {
        for id in rule_definitions(&read(page)) {
            if !page.starts_with("docs/reference/") {
                problems.push(format!("{page}: rule {id} defined outside docs/reference"));
            } else if rules.contains(&id) {
                problems.push(format!("{page}: rule {id} defined twice"));
            } else {
                rules.push(id);
            }
        }
    }
    let mut live: Vec<&str> = pages.iter().map(String::as_str).filter(|p| !frozen(p)).collect();
    if !live.contains(&"AGENTS.md") {
        live.push("AGENTS.md");
    }
    for page in live {
        let text = read(page);
        check_links(root, page, &text, &mut problems);
        for command in commands_in(&text) {
            if !commands.contains(&command) {
                problems.push(format!("{page}: unknown command udtk {command}"));
            }
        }
        let own = rule_definitions(&text);
        for id in prose_lines(&text).into_iter().flat_map(rule_citations) {
            if !rules.contains(&id) && !own.contains(&id) {
                problems.push(format!("{page}: unknown rule {id}"));
            }
        }
        let lines = text.lines().count();
        if page.starts_with("docs/reference/") && lines > REFERENCE_LINE_LIMIT {
            problems.push(format!("{page}: {lines} lines, the limit is {REFERENCE_LINE_LIMIT}"));
        }
    }
    check_code(root, &rules, &mut problems);
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// A throwaway repository: `(path, text)` pairs under a unique temp directory.
    fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("udtk-book-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        root
    }

    fn only(problems: &[String], needle: &str) -> bool {
        problems.len() == 1 && problems[0].contains(needle)
    }

    #[test]
    fn summary_keeps_order_parts_and_repo_paths() {
        let entries = parse_summary(
            "# Summary\n\n[Shelf](../README.md)\n\n# The Book\n\n- [One](book/one.md)\n  - [Two](book/two.md)\n",
        );
        let got: Vec<_> = entries.iter().map(|e| (e.title.as_str(), e.path.as_str(), e.part.as_deref())).collect();
        assert_eq!(
            got,
            vec![
                ("Shelf", "README.md", None),
                ("One", "docs/book/one.md", Some("The Book")),
                ("Two", "docs/book/two.md", Some("The Book")),
            ]
        );
    }

    #[test]
    fn slug_matches_github_headings() {
        assert_eq!(slug("H2. The plan (former 10.1 to 10.8)"), "h2-the-plan-former-101-to-108");
        assert_eq!(slug("The `udtk` book: a_b-c"), "the-udtk-book-a_b-c");
    }

    #[test]
    fn anchors_include_code_spans_duplicates_and_rules() {
        let got = anchors("# A `b`\n\n## Same\n\n## Same\n\nr[storage.no-raw-db]\nText.\n\n```text\n# not a heading\nr[in.code]\n```\n");
        assert_eq!(got, vec!["a-b", "same", "same-1", "r-storage.no-raw-db"]);
    }

    #[test]
    fn paths_normalize_and_relativize() {
        assert_eq!(normalize("docs/book", "../reference/a.md"), "docs/reference/a.md");
        assert_eq!(normalize("docs", "../README.md"), "README.md");
        assert_eq!(relative("docs/book", "docs/reference/a.html"), "../reference/a.html");
        assert_eq!(relative("", "docs/a.html"), "docs/a.html");
        assert_eq!(relative("docs/book", "docs/book/a.html"), "a.html");
    }

    #[test]
    fn links_go_to_pages_or_to_the_repository() {
        let rendered = pages(&["README.md", "docs/book/one.md", "docs/reference/a.md"]);
        let link = |dest| rewrite_link(dest, "docs/book/one.md", "docs/book", &rendered);
        assert_eq!(link("../reference/a.md#r-x.y"), "../reference/a.html#r-x.y");
        assert_eq!(link("../../README.md"), "../../index.html");
        assert_eq!(link("#here"), "#here");
        assert_eq!(link("https://example.com/a.md"), "https://example.com/a.md");
        assert_eq!(
            link("../../opt/cli/main.rs"),
            "https://github.com/avariiiiiiiiii/UniDTK/blob/main/opt/cli/main.rs"
        );
        assert_eq!(
            link("../sessions.md#x"),
            "https://github.com/avariiiiiiiiii/UniDTK/blob/main/docs/sessions.md#x"
        );
        // A stem that merely contains ".md" or ends in README stays intact.
        assert_eq!(rewrite_link("a.mdx", "docs/a.md", "docs", &pages(&["docs/a.mdx"])), "a.mdx");
    }

    #[test]
    fn body_has_heading_ids_rule_anchors_and_no_front_matter() {
        let html = render_body("---\ntitle: x\n---\n\n# Hello `you`\n\nr[a.b]\nRule text.\n", "docs/a.md", "docs", &[]);
        assert!(html.contains("<h1 id=\"hello-you\">"), "{html}");
        assert!(html.contains("id=\"r-a.b\""), "{html}");
        assert!(!html.contains("title: x"), "{html}");
    }

    #[test]
    fn build_writes_pages_in_order_and_drops_stale_output() {
        let root = repo(
            "build",
            &[
                ("README.md", "# Shelf\n"),
                ("docs/SUMMARY.md", "[Shelf](../README.md)\n\n# B\n\n- [One](book/one.md)\n- [Two](book/two.md)\n- [Log](history/log.md)\n"),
                ("docs/book/one.md", "# One\n"),
                ("docs/book/two.md", "# Two\n"),
            ],
        );
        let output = root.join("out");
        std::fs::create_dir_all(output.join("docs")).unwrap();
        std::fs::write(output.join("docs/stale.html"), "old").unwrap();

        let index = build(&root, &output).unwrap();

        assert_eq!(index, output.join("index.html"));
        assert!(!output.join("docs/stale.html").exists());
        let one = std::fs::read_to_string(output.join("docs/book/one.html")).unwrap();
        assert!(one.contains("href=\"two.html\""), "next link: {one}");
        assert!(one.contains("href=\"../../index.html\""), "prev link: {one}");
        // A listed page that did not ship points at the repository.
        assert!(one.contains("blob/main/docs/history/log.md"), "{one}");
        let print = std::fs::read_to_string(output.join("print.html")).unwrap();
        assert!(print.find("One").unwrap() < print.find("Two").unwrap());
    }

    #[test]
    fn check_passes_a_clean_book() {
        let root = repo(
            "clean",
            &[
                ("README.md", "# Shelf\n\nRun `udtk doc`.\n"),
                ("AGENTS.md", "# Rules\n"),
                ("docs/SUMMARY.md", "[Shelf](../README.md)\n\n- [A](reference/a.md)\n- [B](book/b.md)\n"),
                ("docs/reference/a.md", "# A\n\nr[a.one]\nThe rule.\n"),
                ("docs/book/b.md", "# B\n\nSee [the rule](../reference/a.md#r-a.one), r[a.one].\n"),
                ("core/x.rs", "// r[a.one], and docs/book/b.md\n"),
            ],
        );
        assert_eq!(check(&root, &pages(&["doc"])), Vec::<String>::new());
    }

    #[test]
    fn check_reports_each_kind_of_rot() {
        let base = [
            ("README.md", "# Shelf\n"),
            ("AGENTS.md", "# Rules\n"),
            ("docs/SUMMARY.md", "[Shelf](../README.md)\n\n- [A](a.md)\n"),
        ];
        let case = |name: &str, extra: &[(&str, &str)]| {
            let mut files = base.to_vec();
            files.retain(|(path, _)| !extra.iter().any(|(p, _)| p == path));
            files.extend_from_slice(extra);
            check(&repo(name, &files), &pages(&["doc"]))
        };

        assert!(only(&case("missing", &[]), "missing page"));
        assert!(only(&case("orphan", &[("docs/a.md", "# A\n"), ("docs/b.md", "# B\n")]), "not in SUMMARY"));
        assert!(only(&case("link", &[("docs/a.md", "[x](gone.md)\n")]), "broken link"));
        assert!(only(&case("anchor", &[("docs/a.md", "# A\n\n[x](#nope)\n")]), "broken anchor"));
        assert!(only(&case("cmd", &[("docs/a.md", "# A\n\nRun `udtk nope`.\n")]), "unknown command"));
        assert!(only(&case("rule", &[("docs/a.md", "# A\n\nr[no.such]\n")]), "defined outside"));
        assert!(only(&case("cite", &[("docs/a.md", "# A\n\nSee r[no.such] here.\n")]), "unknown rule"));
        assert!(only(&case("code", &[("docs/a.md", "# A\n"), ("core/x.rs", "// docs/gone.md\n")]), "core/x.rs"));
        assert!(only(&case("sect", &[("docs/a.md", "# A\n"), ("core/x.rs", "// docs/a.md §5\n")]), "cite a rule"));
    }

    #[test]
    fn check_leaves_imported_code_alone() {
        let problems = check(
            &repo(
                "imported",
                &[
                    ("README.md", "# S\n"),
                    ("AGENTS.md", "# A\n"),
                    ("docs/SUMMARY.md", "[S](../README.md)\n"),
                    ("core/rune/rune-motion/src/lib.rs", "//! (PLAN.md §3) docs/gone.md r[no.such]\n"),
                ],
            ),
            &pages(&["doc"]),
        );
        assert_eq!(problems, Vec::<String>::new());
    }

    #[test]
    fn check_leaves_frozen_history_alone_and_limits_reference_size() {
        let long = format!("# R\n{}", "x\n".repeat(351));
        let problems = check(
            &repo(
                "frozen",
                &[
                    ("README.md", "# S\n"),
                    ("AGENTS.md", "# A\n"),
                    ("docs/SUMMARY.md", "[S](../README.md)\n\n- [R](reference/r.md)\n- [H](history/h.md)\n"),
                    ("docs/reference/r.md", long.as_str()),
                    ("docs/history/h.md", "[old](gone.md) `udtk removed`\n"),
                ],
            ),
            &pages(&["doc"]),
        );
        assert!(only(&problems, "350"), "{problems:?}");
    }
}
