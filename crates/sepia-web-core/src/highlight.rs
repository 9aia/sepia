//! Keyword-level syntax highlighting for fenced code blocks — a
//! deliberately tiny tokenizer, not a parser. The alternative was a
//! syntect dependency (~200 crates of grammars); for transcript code
//! fences, comments/strings/numbers/keywords in the handful of
//! languages agents actually emit covers the visible 90%.
//!
//! Output is a flat `Vec<Token>` — `sepia-web`'s `CodeBlock` maps
//! `TokenKind::class()` onto Catppuccin token colors and renders one
//! `<span>` per token inside the same `<pre>` it always emitted.

/// What a byte range of the source is. `Plain` carries no class —
/// it renders as an unstyled `<span>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    Str,
    Num,
    Comment,
}

impl TokenKind {
    /// The Tailwind class the token's `<span>` gets — Catppuccin
    /// tokens: `info` for keywords, `success` for strings, `warning`
    /// for numbers, muted italic for comments. `Plain` renders
    /// unstyled (the surrounding `<pre>`'s color).
    pub fn class(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::Keyword => "text-info",
            Self::Str => "text-success",
            Self::Num => "text-warning",
            Self::Comment => "italic text-muted-foreground",
        }
    }
}

/// One contiguous run of `kind`-classified source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
}

/// A language's lexing rules — comment delimiters, string openers,
/// and a keyword set (matched case-insensitively, so `SELECT` and
/// `select` both land).
struct Spec {
    /// Rest-of-line comment openers (`//`, `#`, `--`).
    line_comments: &'static [&'static str],
    /// `(open, close)` pairs (`/* */`, `<!-- -->`).
    block_comments: &'static [(&'static str, &'static str)],
    /// String delimiters, tried longest-first (`"""` before `"`).
    /// A delimiter that also closes the string.
    strings: &'static [&'static str],
    keywords: &'static [&'static str],
}

const LINE_SLASH: &[&str] = &["//"];
const LINE_HASH: &[&str] = &["#"];
const BLOCK_C: &[(&str, &str)] = &[("/*", "*/")];
const DQ_SQUOT: &[&str] = &["\"", "'"];
const DQ_SQUOT_BT: &[&str] = &["\"", "'", "`"];

const RUST: Spec = Spec {
    line_comments: LINE_SLASH,
    block_comments: BLOCK_C,
    // `'` is left out: lifetimes (`&'a str`) would eat the rest of
    // the line hunting a closer. Char literals stay unhighlighted —
    // a fair trade for never mangling borrowed references.
    strings: &["\""],
    keywords: &[
        "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else",
        "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
        "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "yield",
        // Primitive types — not keywords, but they read like them.
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize",
    ],
};

const PYTHON: Spec = Spec {
    line_comments: LINE_HASH,
    block_comments: &[],
    strings: &["\"\"\"", "'''", "\"", "'"],
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "false", "finally", "for", "from", "global", "if", "import",
        "in", "is", "lambda", "none", "nonlocal", "not", "or", "pass", "raise", "return", "true",
        "try", "while", "with", "yield",
    ],
};

const JAVASCRIPT: Spec = Spec {
    line_comments: LINE_SLASH,
    block_comments: BLOCK_C,
    strings: DQ_SQUOT_BT,
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "boolean",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "declare",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "get",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "namespace",
        "new",
        "null",
        "number",
        "of",
        "private",
        "protected",
        "public",
        "readonly",
        "return",
        "set",
        "static",
        "string",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ],
};

const SHELL: Spec = Spec {
    line_comments: LINE_HASH,
    block_comments: &[],
    strings: DQ_SQUOT,
    keywords: &[
        "case", "cd", "do", "done", "echo", "elif", "else", "esac", "export", "fi", "for",
        "function", "if", "in", "local", "readonly", "return", "select", "then", "time", "until",
        "while",
    ],
};

const JSON: Spec = Spec {
    line_comments: &[],
    block_comments: &[],
    strings: &["\""],
    keywords: &["false", "null", "true"],
};

const TOML: Spec = Spec {
    line_comments: LINE_HASH,
    block_comments: &[],
    strings: &["\"\"\"", "'''", "\"", "'"],
    keywords: &["false", "true"],
};

const YAML: Spec = Spec {
    line_comments: LINE_HASH,
    block_comments: &[],
    strings: DQ_SQUOT,
    keywords: &["false", "no", "null", "off", "on", "true", "yes"],
};

const C_LANG: Spec = Spec {
    line_comments: LINE_SLASH,
    block_comments: BLOCK_C,
    strings: DQ_SQUOT,
    keywords: &[
        "auto",
        "bool",
        "break",
        "case",
        "catch",
        "char",
        "class",
        "const",
        "constexpr",
        "continue",
        "default",
        "delete",
        "do",
        "double",
        "else",
        "enum",
        "explicit",
        "extern",
        "false",
        "final",
        "float",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "int",
        "long",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "nullptr",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "register",
        "return",
        "short",
        "signed",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "true",
        "try",
        "typedef",
        "typename",
        "union",
        "unsigned",
        "using",
        "virtual",
        "void",
        "volatile",
        "while",
    ],
};

const GO: Spec = Spec {
    line_comments: LINE_SLASH,
    block_comments: BLOCK_C,
    strings: DQ_SQUOT_BT,
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "false",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "in",
        "interface",
        "map",
        "nil",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "true",
        "type",
        "var",
    ],
};

const JVM: Spec = Spec {
    line_comments: LINE_SLASH,
    block_comments: BLOCK_C,
    strings: DQ_SQUOT,
    keywords: &[
        "abstract",
        "as",
        "boolean",
        "break",
        "byte",
        "case",
        "catch",
        "char",
        "class",
        "companion",
        "const",
        "continue",
        "data",
        "default",
        "do",
        "double",
        "else",
        "enum",
        "extends",
        "false",
        "final",
        "finally",
        "float",
        "for",
        "fun",
        "if",
        "implements",
        "import",
        "in",
        "init",
        "inner",
        "instanceof",
        "int",
        "interface",
        "internal",
        "is",
        "long",
        "native",
        "new",
        "null",
        "object",
        "out",
        "override",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "sealed",
        "short",
        "static",
        "super",
        "suspend",
        "switch",
        "synchronized",
        "this",
        "throw",
        "throws",
        "transient",
        "true",
        "try",
        "val",
        "var",
        "void",
        "volatile",
        "when",
        "while",
    ],
};

const SQL: Spec = Spec {
    line_comments: &["--"],
    block_comments: BLOCK_C,
    strings: &["'"],
    keywords: &[
        "add",
        "all",
        "alter",
        "and",
        "as",
        "asc",
        "between",
        "by",
        "case",
        "check",
        "column",
        "constraint",
        "create",
        "cross",
        "default",
        "delete",
        "desc",
        "distinct",
        "drop",
        "else",
        "end",
        "exists",
        "foreign",
        "from",
        "full",
        "group",
        "having",
        "in",
        "index",
        "inner",
        "insert",
        "into",
        "is",
        "join",
        "key",
        "left",
        "like",
        "limit",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "primary",
        "references",
        "right",
        "select",
        "set",
        "table",
        "then",
        "union",
        "unique",
        "update",
        "values",
        "when",
        "where",
    ],
};

const CSS: Spec = Spec {
    line_comments: &[],
    block_comments: BLOCK_C,
    strings: DQ_SQUOT,
    keywords: &[],
};

/// The spec for a fence's `lang` tag — `None` (and so no
/// highlighting) for empty/unknown/plain-text tags. Matching is
/// case-insensitive and tolerant of `.`-prefixed names.
fn spec_for(lang: &str) -> Option<&'static Spec> {
    let l = lang.trim().trim_start_matches('.').to_ascii_lowercase();
    Some(match l.as_str() {
        "rust" | "rs" => &RUST,
        "python" | "py" | "python3" => &PYTHON,
        "javascript" | "js" | "jsx" | "mjs" | "cjs" | "typescript" | "ts" | "tsx" => &JAVASCRIPT,
        "bash" | "sh" | "shell" | "zsh" | "console" => &SHELL,
        "json" | "jsonl" => &JSON,
        "toml" => &TOML,
        "yaml" | "yml" => &YAML,
        "c" | "h" | "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "objc" => &C_LANG,
        "go" | "golang" => &GO,
        "java" | "kotlin" | "kt" | "scala" | "swift" | "cs" | "csharp" | "c#" => &JVM,
        "sql" => &SQL,
        "css" | "scss" | "less" => &CSS,
        _ => return None,
    })
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Read the length of a quoted string starting at `rest` (which begins
/// with `open`/`close` delimiter `d`). `'`/`"` strings may not cross a
/// newline — an unterminated one yields `None` so the opener falls back
/// to plain text (lone apostrophes in prose-ish code stay sane).
/// `"""`/`'''`/backtick strings may span lines.
fn string_len(rest: &str, d: &str) -> Option<usize> {
    let multiline = d.len() > 1 || d == "`";
    let mut i = d.len();
    while i < rest.len() {
        if rest[i..].starts_with(d) {
            return Some(i + d.len());
        }
        let c = rest[i..].chars().next()?;
        if c == '\\' {
            // Skip the escapee too — `\"` can't close the string.
            i += 1 + rest[i + 1..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        if c == '\n' && !multiline {
            return None;
        }
        i += c.len_utf8();
    }
    // Unterminated at EOF still counts as a string — it *looks* like
    // one all the way down.
    if i == rest.len() && i > d.len() {
        return Some(i);
    }
    None
}

/// Emit a classified span, flushing accumulated plain text first so
/// neighboring plain runs coalesce into one token.
fn push_tok(kind: TokenKind, text: &str, plain: &mut String, toks: &mut Vec<Token>) {
    if !plain.is_empty() {
        toks.push(Token {
            kind: TokenKind::Plain,
            text: std::mem::take(plain),
        });
    }
    if !text.is_empty() {
        toks.push(Token {
            kind,
            text: text.to_string(),
        });
    }
}

/// Tokenize `code` under `lang`'s spec. Unknown languages return a
/// single `Plain` token covering the input — the caller renders it
/// exactly like the old unhighlighted path.
pub fn highlight(lang: &str, code: &str) -> Vec<Token> {
    let Some(spec) = spec_for(lang) else {
        return vec![Token {
            kind: TokenKind::Plain,
            text: code.to_string(),
        }];
    };
    let mut toks = Vec::new();
    let mut plain = String::new();

    let mut rest = code;
    while !rest.is_empty() {
        // Line comment — runs to (not including) the newline.
        if spec.line_comments.iter().any(|m| rest.starts_with(m)) {
            let end = rest.find('\n').unwrap_or(rest.len());
            push_tok(TokenKind::Comment, &rest[..end], &mut plain, &mut toks);
            rest = &rest[end..];
            continue;
        }
        // Block comment — to the closer, or EOF.
        if let Some((open, close)) = spec
            .block_comments
            .iter()
            .find(|(o, _)| rest.starts_with(*o))
        {
            let after = &rest[open.len()..];
            let end = after
                .find(*close)
                .map_or(rest.len(), |i| open.len() + i + close.len());
            push_tok(TokenKind::Comment, &rest[..end], &mut plain, &mut toks);
            rest = &rest[end..];
            continue;
        }
        // String — longest opener wins (`"""` before `"`).
        if let Some(d) = spec
            .strings
            .iter()
            .filter(|d| rest.starts_with(**d))
            .max_by_key(|d| d.len())
            && let Some(n) = string_len(rest, d)
        {
            push_tok(TokenKind::Str, &rest[..n], &mut plain, &mut toks);
            rest = &rest[n..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        // Number — digits plus the usual literal soup (`0x`, `_`, `.`,
        // exponent letters). `e+10`/`e-10` signs ride along.
        if c.is_ascii_digit() {
            let mut i = 0;
            for (j, ch) in rest.char_indices() {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '\'') {
                    i = j + ch.len_utf8();
                } else if matches!(ch, '+' | '-')
                    && rest[..j]
                        .chars()
                        .next_back()
                        .is_some_and(|p| matches!(p, 'e' | 'E' | 'p' | 'P'))
                {
                    i = j + 1;
                } else {
                    break;
                }
            }
            push_tok(TokenKind::Num, &rest[..i.max(1)], &mut plain, &mut toks);
            rest = &rest[i.max(1)..];
            continue;
        }
        // Identifier → keyword lookup (case-insensitive so SQL stays
        // pretty); otherwise plain.
        if is_ident_start(c) {
            let mut i = c.len_utf8();
            while let Some(ch) = rest[i..].chars().next() {
                if !is_ident_char(ch) {
                    break;
                }
                i += ch.len_utf8();
            }
            let word = &rest[..i];
            if spec.keywords.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                push_tok(TokenKind::Keyword, word, &mut plain, &mut toks);
            } else {
                plain.push_str(word);
            }
            rest = &rest[i..];
            continue;
        }
        // Anything else is punctuation/whitespace — plain.
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if !plain.is_empty() {
        toks.push(Token {
            kind: TokenKind::Plain,
            text: plain,
        });
    }
    toks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, code: &str) -> Vec<(TokenKind, String)> {
        highlight(lang, code)
            .into_iter()
            .map(|t| (t.kind, t.text))
            .collect()
    }

    #[test]
    fn unknown_lang_is_one_plain_token() {
        assert_eq!(
            kinds("brainfuck", "+++"),
            vec![(TokenKind::Plain, "+++".to_string())]
        );
        assert_eq!(
            kinds("", "x = 1"),
            vec![(TokenKind::Plain, "x = 1".to_string())]
        );
        // Aliases + case + dot prefixes normalize.
        assert!(matches!(
            highlight(".RS", "fn").first(),
            Some(t) if t.kind == TokenKind::Keyword
        ));
    }

    #[test]
    fn rust_tokens() {
        let toks = kinds("rust", "let x = 42; // hi\n\"s\"");
        assert!(toks.contains(&(TokenKind::Keyword, "let".to_string())));
        assert!(toks.contains(&(TokenKind::Num, "42".to_string())));
        assert!(toks.contains(&(TokenKind::Comment, "// hi".to_string())));
        assert!(toks.contains(&(TokenKind::Str, "\"s\"".to_string())));
    }

    #[test]
    fn rust_lifetimes_dont_eat_lines() {
        // `'` isn't a string opener for rust — the `'a` lifetime and
        // the keyword after it both survive.
        let toks = kinds("rust", "fn f<'a>(x: &'a str) -> bool { true }");
        assert!(toks.contains(&(TokenKind::Keyword, "fn".to_string())));
        assert!(toks.contains(&(TokenKind::Keyword, "bool".to_string())));
        assert!(!toks.iter().any(|(k, _)| *k == TokenKind::Str));
    }

    #[test]
    fn strings_respect_escapes_and_newlines() {
        let toks = kinds("js", "let s = \"a\\\"b\";");
        assert!(toks.contains(&(TokenKind::Str, "\"a\\\"b\"".to_string())));
        // Unterminated at newline → the quote is plain, `after` isn't
        // swallowed into a string.
        let toks = kinds("js", "let s = \"oops\nlet t = 1;");
        assert!(toks.contains(&(TokenKind::Keyword, "let".to_string())));
        assert_eq!(toks.iter().filter(|(k, _)| *k == TokenKind::Str).count(), 0);
    }

    #[test]
    fn python_comments_and_keywords() {
        let toks = kinds("python", "# note\ndef f():\n    return None");
        assert!(toks.contains(&(TokenKind::Comment, "# note".to_string())));
        assert!(toks.contains(&(TokenKind::Keyword, "def".to_string())));
        assert!(toks.contains(&(TokenKind::Keyword, "None".to_string())));
    }

    #[test]
    fn block_comments_span_lines() {
        let toks = kinds("c", "int x; /* a\nb */ int y;");
        assert!(toks.contains(&(TokenKind::Comment, "/* a\nb */".to_string())));
        assert!(toks.contains(&(TokenKind::Keyword, "int".to_string())));
    }

    #[test]
    fn shell_hash_but_not_mid_word() {
        let toks = kinds("bash", "echo hi # done");
        assert!(toks.contains(&(TokenKind::Comment, "# done".to_string())));
        assert!(toks.contains(&(TokenKind::Keyword, "echo".to_string())));
    }

    #[test]
    fn sql_is_case_insensitive() {
        let toks = kinds("sql", "SELECT * FROM t WHERE x = 'a'");
        assert!(toks.contains(&(TokenKind::Keyword, "SELECT".to_string())));
        assert!(toks.contains(&(TokenKind::Str, "'a'".to_string())));
    }

    #[test]
    fn numbers_carry_literal_suffixes() {
        let toks = kinds("rust", "let n = 0x_ff + 1e-3 + 4u64;");
        assert!(toks.contains(&(TokenKind::Num, "0x_ff".to_string())));
        assert!(toks.contains(&(TokenKind::Num, "1e-3".to_string())));
        assert!(toks.contains(&(TokenKind::Num, "4u64".to_string())));
    }
}
