//! Скрипты, которые браузер встраивает в каждый документ: без строк-комментариев.
//!
//! Комментарии нужны в исходнике, а не в каждом документе каждой вкладки.
//! Строка, которая начинается с `//`, — комментарий, только если она не лежит
//! внутри многострочной шаблонной строки (`` `…` ``): там это текст, и
//! выбросить его значило бы молча поменять скрипт.

/// Скрипт без строк, целиком занятых комментарием `//`.
pub fn strip_comment_lines(source: &str) -> String {
    let mut out = Vec::new();
    let mut template = false;
    for line in source.lines() {
        if !template && line.trim_start().starts_with("//") {
            continue;
        }
        template = ends_inside_template(line, template);
        out.push(line);
    }
    out.join("\n")
}

/// Остаётся ли шаблонная строка открытой после этой строки исходника. Кавычки
/// `'` и `"` строк не переходят, после `//` вне строк — комментарий до конца
/// строки, `\` экранирует следующий символ.
fn ends_inside_template(line: &str, mut template: bool) -> bool {
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '`' if quote.is_none() => template = !template,
            '\'' | '"' if !template => match quote {
                Some(open) if open == ch => quote = None,
                None => quote = Some(ch),
                Some(_) => {}
            },
            '/' if !template && quote.is_none() && chars.peek() == Some(&'/') => break,
            _ => {}
        }
    }
    template
}

#[cfg(test)]
mod tests {
    use super::strip_comment_lines;

    #[test]
    fn comment_lines_are_dropped() {
        assert_eq!(
            strip_comment_lines("// комментарий\nconst a = 1;\n  // ещё\n  a;"),
            "const a = 1;\n  a;"
        );
    }

    #[test]
    fn template_text_is_kept() {
        let source = "const css = `\n// не комментарий\n.x{}\n`;\n// комментарий\nf(css);";
        assert_eq!(
            strip_comment_lines(source),
            "const css = `\n// не комментарий\n.x{}\n`;\nf(css);"
        );
    }

    #[test]
    fn quotes_and_escapes_do_not_open_templates() {
        // Обратная кавычка в строке, экранированная — в шаблоне, `//` в адресе
        // после кавычки и в комментарии после кода.
        let source = "const q = \"`\";\n// drop\nconst t = `a\\`\n// keep\n`;\nconst u = \"https://x\"; // `\n// drop";
        assert_eq!(
            strip_comment_lines(source),
            "const q = \"`\";\nconst t = `a\\`\n// keep\n`;\nconst u = \"https://x\"; // `"
        );
    }
}
