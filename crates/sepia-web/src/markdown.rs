//! Markdown-lite — deliberately tiny: fenced code blocks, ATX headings,
//! `-`/`*`/ordered lists, paragraphs, and `` `code` ``, `**bold**`,
//! `*em*`, `[text](url)` inline. Everything else renders as literal
//! text. Not a CommonMark implementation.

use leptos::prelude::*;

/// A parsed block.
#[derive(Clone, Debug, PartialEq)]
pub enum MdBlock {
    Heading(u8, Vec<MdInline>),
    Paragraph(Vec<MdInline>),
    /// ` ```lang …``` ` fence.
    Code {
        lang: Option<String>,
        code: String,
    },
    List {
        ordered: bool,
        items: Vec<Vec<MdInline>>,
    },
}

/// Inline span.
#[derive(Clone, Debug, PartialEq)]
pub enum MdInline {
    Text(String),
    Code(String),
    Strong(String),
    Em(String),
    Link { text: String, href: String },
}

/// Parse a message into blocks.
pub fn parse(input: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Option<(bool, Vec<Vec<MdInline>>)> = None;

    let flush_para = |para: &mut Vec<String>, blocks: &mut Vec<MdBlock>| {
        if !para.is_empty() {
            blocks.push(MdBlock::Paragraph(inlines(&para.join("\n"))));
            para.clear();
        }
    };
    let flush_list = |list: &mut Option<(bool, Vec<Vec<MdInline>>)>, blocks: &mut Vec<MdBlock>| {
        if let Some((ordered, items)) = list.take() {
            blocks.push(MdBlock::List { ordered, items });
        }
    };

    let mut lines = input.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
            let lang = (!fence.trim().is_empty()).then(|| fence.trim().to_string());
            let mut code = String::new();
            for l in lines.by_ref() {
                if l.trim_end() == "```" {
                    break;
                }
                code.push_str(l);
                code.push('\n');
            }
            blocks.push(MdBlock::Code {
                lang,
                code: code.trim_end_matches('\n').to_string(),
            });
            continue;
        }

        let heading = trimmed
            .strip_prefix("### ")
            .map(|_| 3)
            .or_else(|| trimmed.strip_prefix("## ").map(|_| 2))
            .or_else(|| trimmed.strip_prefix("# ").map(|_| 1));
        if let Some(level) = heading {
            flush_para(&mut para, &mut blocks);
            flush_list(&mut list, &mut blocks);
            let text = trimmed.trim_start_matches('#').trim_start();
            blocks.push(MdBlock::Heading(level, inlines(text)));
            continue;
        }

        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));
        let ordered_item = || {
            let (num, rest) = trimmed.split_once('.')?;
            if rest.starts_with(' ') && num.bytes().all(|b| b.is_ascii_digit()) && !num.is_empty() {
                Some(rest.trim_start())
            } else {
                None
            }
        };
        if let Some(item) = bullet {
            flush_para(&mut para, &mut blocks);
            if let Some((false, items)) = &mut list {
                items.push(inlines(item));
            } else {
                flush_list(&mut list, &mut blocks);
                list = Some((false, vec![inlines(item)]));
            }
            continue;
        }
        if let Some(item) = ordered_item() {
            flush_para(&mut para, &mut blocks);
            if let Some((true, items)) = &mut list {
                items.push(inlines(item));
            } else {
                flush_list(&mut list, &mut blocks);
                list = Some((true, vec![inlines(item)]));
            }
            continue;
        }

        flush_list(&mut list, &mut blocks);
        if trimmed.is_empty() {
            flush_para(&mut para, &mut blocks);
        } else {
            para.push(trimmed.to_string());
        }
    }
    flush_para(&mut para, &mut blocks);
    flush_list(&mut list, &mut blocks);
    blocks
}

/// Inline spans — scans for `` ` ``, `**`, `*`, `[..](..)`; unmatched
/// markers stay literal.
fn inlines(text: &str) -> Vec<MdInline> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<MdInline>| {
        if !plain.is_empty() {
            out.push(MdInline::Text(std::mem::take(plain)));
        }
    };
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('`') {
            if let Some(end) = inner.find('`') {
                flush(&mut plain, &mut out);
                out.push(MdInline::Code(inner[..end].to_string()));
                rest = &inner[end + 1..];
                continue;
            }
        }
        if let Some(inner) = rest.strip_prefix("**") {
            if let Some(end) = inner.find("**") {
                if !inner[..end].is_empty() {
                    flush(&mut plain, &mut out);
                    out.push(MdInline::Strong(inner[..end].to_string()));
                    rest = &inner[end + 2..];
                    continue;
                }
            }
        }
        if let Some(inner) = rest.strip_prefix('*') {
            if let Some(end) = inner.find('*') {
                if !inner[..end].is_empty() {
                    flush(&mut plain, &mut out);
                    out.push(MdInline::Em(inner[..end].to_string()));
                    rest = &inner[end + 1..];
                    continue;
                }
            }
        }
        if let Some(inner) = rest.strip_prefix('[') {
            if let Some(close) = inner.find("](") {
                if let Some(pend) = inner[close + 2..].find(')') {
                    let label = &inner[..close];
                    let href = &inner[close + 2..close + 2 + pend];
                    flush(&mut plain, &mut out);
                    out.push(MdInline::Link {
                        text: label.to_string(),
                        href: href.to_string(),
                    });
                    rest = &inner[close + 2 + pend + 1..];
                    continue;
                }
            }
        }
        // Consume one char (multi-byte safe).
        let ch_len = rest.chars().next().map_or(1, char::len_utf8);
        plain.push_str(&rest[..ch_len]);
        rest = &rest[ch_len..];
    }
    flush(&mut plain, &mut out);
    out
}

fn render_inline(span: MdInline) -> impl IntoView {
    match span {
        MdInline::Text(t) => view! { <span>{t}</span> }.into_any(),
        MdInline::Code(c) => view! { <code class="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em] text-info">{c}</code> }.into_any(),
        MdInline::Strong(s) => view! { <strong>{s}</strong> }.into_any(),
        MdInline::Em(s) => view! { <em>{s}</em> }.into_any(),
        MdInline::Link { text, href } => view! {
            <a href=href target="_blank" rel="noreferrer noopener">{text}</a>
        }
        .into_any(),
    }
}

fn render_block(block: MdBlock) -> impl IntoView {
    match block {
        MdBlock::Heading(level, spans) => {
            let inner = spans.into_iter().map(render_inline).collect::<Vec<_>>();
            match level {
                1 => view! { <h3>{inner}</h3> }.into_any(),
                2 => view! { <h4>{inner}</h4> }.into_any(),
                _ => view! { <h5>{inner}</h5> }.into_any(),
            }
        }
        MdBlock::Paragraph(spans) => {
            view! { <p>{spans.into_iter().map(render_inline).collect::<Vec<_>>()}</p> }.into_any()
        }
        MdBlock::Code { lang, code } => view! { <CodeBlock lang=lang code=code/> }.into_any(),
        MdBlock::List { ordered, items } => {
            let items = items
                .into_iter()
                .map(|item| {
                    view! { <li>{item.into_iter().map(render_inline).collect::<Vec<_>>()}</li> }
                })
                .collect::<Vec<_>>();
            if ordered {
                view! { <ol>{items}</ol> }.into_any()
            } else {
                view! { <ul>{items}</ul> }.into_any()
            }
        }
    }
}

/// Fenced code block — a header strip carries the copy affordance and
/// the right-aligned language badge; the `<pre>` sits below it.
#[allow(clippy::needless_pass_by_value)] // component props are owned
#[component]
fn CodeBlock(#[prop(into)] lang: Option<String>, #[prop(into)] code: String) -> impl IntoView {
    let copied = RwSignal::new(false);
    // `use_timeout_fn` is SSR-safe — its callback never fires server-side.
    let reset = leptos_use::use_timeout_fn(move |()| copied.set(false), 1_500.0).start;
    let on_copy = {
        let code = code.clone();
        move |_| {
            copy_to_clipboard(&code);
            copied.set(true);
            reset(());
        }
    };
    view! {
        <div class="overflow-hidden rounded-md border bg-secondary/60">
            <div class="flex items-center justify-end gap-2 border-b border-border/60 px-3 py-1">
                {lang.map(|l| view! {
                    <span class="font-mono text-[10px] uppercase tracking-wide text-muted-foreground">
                        {l}
                    </span>
                })}
                <button
                    type="button"
                    aria-label="Copy code"
                    class="rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    on:click=on_copy
                >
                    {move || if copied.get() { "copied" } else { "copy" }}
                </button>
            </div>
            <pre class="overflow-x-auto p-3 font-mono text-xs leading-relaxed"><code>{code}</code></pre>
        </div>
    }
}

/// Renders `text` as markdown-lite.
#[allow(clippy::needless_pass_by_value)] // component props are owned
#[component]
pub fn Markdown(#[prop(into)] text: String) -> impl IntoView {
    let blocks = parse(&text);
    view! {
        <div class="prose dark:prose-invert prose-sm max-w-none text-foreground prose-headings:text-foreground prose-p:my-2 prose-a:text-info prose-strong:text-foreground">
            {blocks.into_iter().map(render_block).collect::<Vec<_>>()}
        </div>
    }
}

// Kept away from the `impl IntoView` items above — the `check` xtask
// lints a cfg attr adjacent to markup (`view!`/`impl IntoView`).

/// Clipboard write — browser-only. The ssr stub keeps the `on:click`
/// handler compiling; it emits no markup, so SSR/hydrate agree.
#[cfg(feature = "hydrate")]
fn copy_to_clipboard(text: &str) {
    let _ = window().navigator().clipboard().write_text(text);
}

#[cfg(not(feature = "hydrate"))]
fn copy_to_clipboard(_: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_and_code_fence() {
        let md = parse("hello world\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(
            md,
            vec![
                MdBlock::Paragraph(vec![MdInline::Text("hello world".into())]),
                MdBlock::Code {
                    lang: Some("rust".into()),
                    code: "fn main() {}".into()
                }
            ]
        );
    }

    #[test]
    fn headings_and_lists() {
        let md = parse("# Title\n\n- one\n- two\n\n1. a\n2. b\n");
        assert_eq!(
            md,
            vec![
                MdBlock::Heading(1, vec![MdInline::Text("Title".into())]),
                MdBlock::List {
                    ordered: false,
                    items: vec![
                        vec![MdInline::Text("one".into())],
                        vec![MdInline::Text("two".into())]
                    ]
                },
                MdBlock::List {
                    ordered: true,
                    items: vec![
                        vec![MdInline::Text("a".into())],
                        vec![MdInline::Text("b".into())]
                    ]
                }
            ]
        );
    }

    #[test]
    fn inline_spans() {
        let md = inlines("see `x` and **y** but *z* and [a](https://b) done");
        assert_eq!(
            md,
            vec![
                MdInline::Text("see ".into()),
                MdInline::Code("x".into()),
                MdInline::Text(" and ".into()),
                MdInline::Strong("y".into()),
                MdInline::Text(" but ".into()),
                MdInline::Em("z".into()),
                MdInline::Text(" and ".into()),
                MdInline::Link {
                    text: "a".into(),
                    href: "https://b".into()
                },
                MdInline::Text(" done".into()),
            ]
        );
    }

    #[test]
    fn unmatched_markers_stay_literal() {
        assert_eq!(inlines("a `b c"), vec![MdInline::Text("a `b c".into())]);
        assert_eq!(inlines("**x"), vec![MdInline::Text("**x".into())]);
        assert_eq!(inlines("**x"), vec![MdInline::Text("**x".into())]);
    }
}
