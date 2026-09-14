//! Файл закладок в формате Netscape — `bookmarks.html`.
//!
//! Этот формат экспортируют и читают все браузеры: Chrome, Edge, Firefox,
//! Яндекс, Opera. Настоящим HTML он не является — теги `<DT>` и `<p>` не
//! закрываются, регистр плавает, — поэтому здесь не HTML-парсер, а разбор
//! ровно тех конструкций, из которых файл состоит:
//!
//! ```text
//! <DL><p>
//!     <DT><H3 PERSONAL_TOOLBAR_FOLDER="true">Панель закладок</H3>
//!     <DL><p>
//!         <DT><A HREF="https://…" ADD_DATE="…" ICON="data:…">Заголовок</A>
//!     </DL><p>
//! </DL><p>
//! ```

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRole {
    Plain,
    /// Панель закладок исходного браузера.
    Toolbar,
    /// «Другие закладки» (Firefox выносит их отдельной папкой).
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportNode {
    Folder {
        title: String,
        role: FolderRole,
        added_at: Option<i64>,
        children: Vec<ImportNode>,
    },
    Link {
        title: String,
        url: String,
        icon: String,
        added_at: Option<i64>,
    },
}

struct Level {
    folder: Option<PendingFolder>,
    children: Vec<ImportNode>,
}

struct PendingFolder {
    title: String,
    role: FolderRole,
    added_at: Option<i64>,
}

/// Разобрать файл закладок. Всё непонятное пропускается: лучше импортировать
/// девять закладок из десяти, чем ни одной.
pub fn parse(html: &str) -> Vec<ImportNode> {
    let mut stack = vec![Level {
        folder: None,
        children: Vec::new(),
    }];
    let mut pending: Option<PendingFolder> = None;
    let mut root_seen = false;
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];

        if let Some(comment) = rest.strip_prefix("!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }

        let closing = rest.starts_with('/');
        let name_start = usize::from(closing);
        let name_end = rest[name_start..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map_or(rest.len(), |i| i + name_start);
        let name = rest[name_start..name_end].to_ascii_lowercase();

        let Some(gt) = tag_end(&rest[name_end..]).map(|i| i + name_end) else {
            break;
        };
        let attrs = attributes(&rest[name_end..gt]);
        rest = &rest[gt + 1..];

        match (closing, name.as_str()) {
            (false, "h3") => {
                let (text, after) = inner_text(rest, "h3");
                rest = after;
                pending = Some(PendingFolder {
                    title: decode(text.trim()),
                    role: if flag(&attrs, "personal_toolbar_folder") {
                        FolderRole::Toolbar
                    } else if flag(&attrs, "unfiled_bookmarks_folder") {
                        FolderRole::Other
                    } else {
                        FolderRole::Plain
                    },
                    added_at: number(&attrs, "add_date"),
                });
            }
            (false, "a") => {
                let (text, after) = inner_text(rest, "a");
                rest = after;
                let url = attr(&attrs, "href").map(decode).unwrap_or_default();
                // Ярлыки на javascript: и «place:»-запросы Firefox — не закладки.
                if url.is_empty() || url.starts_with("place:") || url.starts_with("javascript:") {
                    continue;
                }
                let title = decode(text.trim());
                if let Some(level) = stack.last_mut() {
                    level.children.push(ImportNode::Link {
                        title: if title.is_empty() { url.clone() } else { title },
                        url,
                        icon: attr(&attrs, "icon").map(decode).unwrap_or_default(),
                        added_at: number(&attrs, "add_date"),
                    });
                }
            }
            (false, "dl") => {
                if let Some(folder) = pending.take() {
                    stack.push(Level {
                        folder: Some(folder),
                        children: Vec::new(),
                    });
                } else if root_seen {
                    stack.push(Level {
                        folder: None,
                        children: Vec::new(),
                    });
                } else {
                    root_seen = true;
                }
            }
            (true, "dl") => {
                if let Some(folder) = pending.take() {
                    attach(&mut stack, Some(folder), Vec::new());
                }
                if stack.len() > 1 {
                    let level = stack.pop().expect("уровень есть");
                    attach(&mut stack, level.folder, level.children);
                }
            }
            _ => {}
        }
    }

    if let Some(folder) = pending.take() {
        attach(&mut stack, Some(folder), Vec::new());
    }
    while stack.len() > 1 {
        let level = stack.pop().expect("уровень есть");
        attach(&mut stack, level.folder, level.children);
    }
    stack.pop().map(|level| level.children).unwrap_or_default()
}

fn attach(stack: &mut [Level], folder: Option<PendingFolder>, children: Vec<ImportNode>) {
    let Some(parent) = stack.last_mut() else {
        return;
    };
    match folder {
        Some(folder) => parent.children.push(ImportNode::Folder {
            title: folder.title,
            role: folder.role,
            added_at: folder.added_at,
            children,
        }),
        // Безымянный вложенный <DL> — просто продолжение текущей папки.
        None => parent.children.extend(children),
    }
}

/// Конец тега с учётом кавычек: в `ICON="data:…"` могут встретиться любые символы.
fn tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Текст до закрывающего тега, без учёта регистра. Возвращает текст и хвост
/// после закрывающего тега.
fn inner_text<'a>(text: &'a str, tag: &str) -> (&'a str, &'a str) {
    let mut search = 0;
    while let Some(found) = text[search..].find("</") {
        let start = search + found;
        let name = &text[start + 2..];
        let matches = name.len() >= tag.len()
            && name[..tag.len()].eq_ignore_ascii_case(tag)
            && name[tag.len()..]
                .chars()
                .next()
                .is_none_or(|c| c == '>' || c.is_whitespace());
        if matches {
            let after = name.find('>').map_or("", |gt| &name[gt + 1..]);
            return (&text[..start], after);
        }
        search = start + 2;
    }
    (text, "")
}

fn attributes(source: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut chars = source.char_indices().peekable();

    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() || c == '/' {
            chars.next();
            continue;
        }
        let mut end = start;
        while let Some(&(index, c)) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            end = index + c.len_utf8();
            chars.next();
        }
        let name = source[start..end].to_ascii_lowercase();

        while matches!(chars.peek(), Some((_, c)) if c.is_whitespace()) {
            chars.next();
        }
        if !matches!(chars.peek(), Some((_, '='))) {
            result.push((name, String::new()));
            continue;
        }
        chars.next();
        while matches!(chars.peek(), Some((_, c)) if c.is_whitespace()) {
            chars.next();
        }

        let value = match chars.peek() {
            Some(&(index, quote @ ('"' | '\''))) => {
                chars.next();
                let from = index + 1;
                let mut to = source.len();
                for (i, c) in chars.by_ref() {
                    if c == quote {
                        to = i;
                        break;
                    }
                }
                &source[from..to.max(from)]
            }
            Some(&(index, _)) => {
                let mut to = source.len();
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_whitespace() {
                        to = i;
                        break;
                    }
                    chars.next();
                }
                &source[index..to]
            }
            None => "",
        };
        result.push((name, value.to_string()));
    }
    result
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn flag(attrs: &[(String, String)], name: &str) -> bool {
    attr(attrs, name).is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn number(attrs: &[(String, String)], name: &str) -> Option<i64> {
    let value: i64 = attr(attrs, name)?.trim().parse().ok()?;
    // Ноль — «дата неизвестна»: так её пишет и наш экспорт, и Chrome.
    if value <= 0 {
        return None;
    }
    // Firefox пишет микросекунды в LAST_MODIFIED, а кто-то и в ADD_DATE.
    Some(if value > 100_000_000_000 {
        value / 1_000_000
    } else {
        value
    })
}

fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|semi| *semi <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Собрать файл так же, как это делает Chrome: панель закладок — папкой с
/// флагом, «Другие закладки» — прямо в корне. Такой файл без вопросов
/// принимает любой браузер.
pub fn render(bar_title: &str, bar: &[ImportNode], other: &[ImportNode]) -> String {
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file.\n     \
         It will be read and overwritten.\n     \
         DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n\
         <H1>Bookmarks</H1>\n\
         <DL><p>\n",
    );
    out.push_str(&format!(
        "    <DT><H3 ADD_DATE=\"0\" LAST_MODIFIED=\"0\" PERSONAL_TOOLBAR_FOLDER=\"true\">{}</H3>\n    <DL><p>\n",
        escape(bar_title)
    ));
    render_nodes(&mut out, bar, 2);
    out.push_str("    </DL><p>\n");
    render_nodes(&mut out, other, 1);
    out.push_str("</DL><p>\n");
    out
}

fn render_nodes(out: &mut String, nodes: &[ImportNode], depth: usize) {
    let pad = "    ".repeat(depth);
    for node in nodes {
        match node {
            ImportNode::Link {
                title,
                url,
                icon,
                added_at,
            } => {
                out.push_str(&format!(
                    "{pad}<DT><A HREF=\"{}\" ADD_DATE=\"{}\"",
                    escape(url),
                    added_at.unwrap_or(0)
                ));
                // Иконку сохраняем только встроенную: ссылка на favicon чужого
                // сайта в файле закладок — это запрос на тот сайт при импорте.
                if icon.starts_with("data:") {
                    out.push_str(&format!(" ICON=\"{}\"", escape(icon)));
                }
                out.push_str(&format!(">{}</A>\n", escape(title)));
            }
            ImportNode::Folder {
                title,
                added_at,
                children,
                ..
            } => {
                out.push_str(&format!(
                    "{pad}<DT><H3 ADD_DATE=\"{}\" LAST_MODIFIED=\"0\">{}</H3>\n{pad}<DL><p>\n",
                    added_at.unwrap_or(0),
                    escape(title)
                ));
                render_nodes(out, children, depth + 1);
                out.push_str(&format!("{pad}</DL><p>\n"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHROME: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<!-- This is an automatically generated file.
     It will be read and overwritten.
     DO NOT EDIT! -->
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3 ADD_DATE="1700000000" LAST_MODIFIED="0" PERSONAL_TOOLBAR_FOLDER="true">Панель закладок</H3>
    <DL><p>
        <DT><A HREF="https://habr.com/ru/" ADD_DATE="1700000001" ICON="data:image/png;base64,AAA>">Хабр &amp; Ко</A>
        <DT><H3 ADD_DATE="1700000002">Кино</H3>
        <DL><p>
            <DT><A HREF="https://www.kinopoisk.ru/">Кинопоиск</A>
        </DL><p>
    </DL><p>
    <DT><a href='https://github.com/'>GitHub</a>
</DL><p>
"#;

    #[test]
    fn chrome_export_is_parsed() {
        let tree = parse(CHROME);
        assert_eq!(tree.len(), 2);

        let ImportNode::Folder {
            role,
            children,
            title,
            ..
        } = &tree[0]
        else {
            panic!("первой должна быть панель закладок");
        };
        assert_eq!(*role, FolderRole::Toolbar);
        assert_eq!(title, "Панель закладок");
        assert_eq!(children.len(), 2);

        let ImportNode::Link {
            title, icon, url, ..
        } = &children[0]
        else {
            panic!("ссылка");
        };
        assert_eq!(title, "Хабр & Ко");
        assert_eq!(url, "https://habr.com/ru/");
        assert!(icon.starts_with("data:image/png"));

        let ImportNode::Folder { children: kino, .. } = &children[1] else {
            panic!("вложенная папка");
        };
        assert_eq!(kino.len(), 1);

        assert!(matches!(&tree[1], ImportNode::Link { url, .. } if url == "https://github.com/"));
    }

    #[test]
    fn render_round_trips() {
        let tree = parse(CHROME);
        let ImportNode::Folder { children: bar, .. } = &tree[0] else {
            panic!("панель");
        };
        let html = render("Панель закладок", bar, &tree[1..]);
        let again = parse(&html);
        assert_eq!(again.len(), 2);
        let ImportNode::Folder { children, .. } = &again[0] else {
            panic!("панель");
        };
        assert_eq!(children, bar);
    }

    #[test]
    fn broken_file_does_not_panic() {
        assert!(parse("<DL><p><DT><H3>Без конца").len() == 1);
        assert!(parse("<<<>>> & ; <A HREF=").is_empty());
    }
}
