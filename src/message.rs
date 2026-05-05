use std::collections::HashSet;

pub enum Block {
    Text(String),
    Code(String),
    Voice { duration_secs: u32 },
}

#[derive(Clone)]
pub struct Reaction {
    pub key: String,
    pub users: Vec<String>,
}

pub struct ThreadReply {
    pub time: String,
    pub author: String,
    pub blocks: Vec<Block>,
    pub event_id: String,
}

pub struct Message {
    pub time: String,
    pub author: String,
    pub blocks: Vec<Block>,
    pub replies: Vec<ThreadReply>,
    pub reactions: Vec<Reaction>,
    pub event_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Top,
    Reply,
}

#[derive(Debug, Clone, Copy)]
pub struct ViewItem {
    pub kind: ItemKind,
    pub msg_idx: usize,
    pub reply_idx: usize, // unused for Top
}

pub fn build_visible_items(messages: &[Message], expanded: &HashSet<usize>) -> Vec<ViewItem> {
    let mut out = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        out.push(ViewItem {
            kind: ItemKind::Top,
            msg_idx: i,
            reply_idx: 0,
        });
        if expanded.contains(&i) {
            for j in 0..msg.replies.len() {
                out.push(ViewItem {
                    kind: ItemKind::Reply,
                    msg_idx: i,
                    reply_idx: j,
                });
            }
        }
    }
    out
}

pub use widgets::WrappedLine;

const CONT_INDENT: &str = "  ";
const REPLY_INDENT: &str = "  ";

pub fn wrap_view(
    messages: &[Message],
    items: &[ViewItem],
    expanded: &HashSet<usize>,
    width: u16,
) -> Vec<WrappedLine> {
    let width = width.max(1) as usize;
    let mut out = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let msg = &messages[item.msg_idx];
        match item.kind {
            ItemKind::Top => {
                let indicator = if msg.replies.is_empty() {
                    " "
                } else if expanded.contains(&item.msg_idx) {
                    "-"
                } else {
                    "+"
                };
                let header = format!("{}{} <{}> ", indicator, msg.time, msg.author);
                render_blocks(&mut out, idx, &header, &msg.blocks, width, CONT_INDENT);
            }
            ItemKind::Reply => {
                let r = &msg.replies[item.reply_idx];
                let header = format!("{}{} <{}> ", REPLY_INDENT, r.time, r.author);
                let cont = format!("{}{}", REPLY_INDENT, CONT_INDENT);
                render_blocks(&mut out, idx, &header, &r.blocks, width, &cont);
            }
        }
    }
    out
}

fn render_blocks(
    out: &mut Vec<WrappedLine>,
    item_idx: usize,
    header: &str,
    blocks: &[Block],
    width: usize,
    cont_indent: &str,
) {
    let header_len = header.chars().count();
    let cont_len = cont_indent.chars().count();
    let mut header_emitted = false;

    for block in blocks {
        match block {
            Block::Text(text) => {
                let (first_prefix, first_budget) = if !header_emitted {
                    (header, width.saturating_sub(header_len).max(1))
                } else {
                    (cont_indent, width.saturating_sub(cont_len).max(1))
                };
                let cont_budget = width.saturating_sub(cont_len).max(1);
                let lines = wrap_text_with_budgets(text, first_budget, cont_budget);
                for (j, l) in lines.into_iter().enumerate() {
                    let prefix = if !header_emitted && j == 0 {
                        first_prefix
                    } else {
                        cont_indent
                    };
                    let is_first = !header_emitted && j == 0;
                    out.push(WrappedLine {
                        item_idx,
                        is_first,
                        text: format!("{}{}", prefix, l),
                        ..Default::default()
                    });
                    if is_first {
                        header_emitted = true;
                    }
                }
            }
            Block::Code(code) => {
                if !header_emitted {
                    out.push(WrappedLine {
                        item_idx,
                        is_first: true,
                        text: header.trim_end().to_string(),
                        ..Default::default()
                    });
                    header_emitted = true;
                }
                for line in code.lines() {
                    let truncated: String = line.chars().take(width).collect();
                    out.push(WrappedLine {
                        item_idx,
                        is_first: false,
                        text: truncated,
                        ..Default::default()
                    });
                }
            }
            Block::Voice { duration_secs } => {
                let mins = duration_secs / 60;
                let secs = duration_secs % 60;
                let label = format!("[voix {}:{:02}  ·  v: lire]", mins, secs);
                if !header_emitted {
                    let combined = format!("{}{}", header, label);
                    let chars: String = combined.chars().take(width).collect();
                    out.push(WrappedLine {
                        item_idx,
                        is_first: true,
                        text: chars,
                        ..Default::default()
                    });
                    header_emitted = true;
                } else {
                    let chars: String =
                        format!("{}{}", cont_indent, label).chars().take(width).collect();
                    out.push(WrappedLine {
                        item_idx,
                        is_first: false,
                        text: chars,
                        ..Default::default()
                    });
                }
            }
        }
    }

    if !header_emitted {
        out.push(WrappedLine {
            item_idx,
            is_first: true,
            text: header.trim_end().to_string(),
            ..Default::default()
        });
    }
}

fn wrap_text_with_budgets(text: &str, first_budget: usize, cont_budget: usize) -> Vec<String> {
    let first_budget = first_budget.max(1);
    let cont_budget = cont_budget.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        let on_first = lines.is_empty() && current.is_empty();
        let budget = if on_first { first_budget } else { cont_budget };

        if word_len > budget {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let chars: Vec<char> = word.chars().collect();
            let mut start = 0;
            while start < chars.len() {
                let b = if lines.is_empty() && current.is_empty() {
                    first_budget
                } else {
                    cont_budget
                };
                let end = (start + b).min(chars.len());
                let chunk: String = chars[start..end].iter().collect();
                if end < chars.len() {
                    lines.push(chunk);
                } else {
                    current = chunk;
                    current_len = end - start;
                }
                start = end;
            }
            continue;
        }

        let needed = if current.is_empty() {
            word_len
        } else {
            current_len + 1 + word_len
        };
        if needed <= budget {
            if !current.is_empty() {
                current.push(' ');
                current_len += 1;
            }
            current.push_str(word);
            current_len += word_len;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_len = word_len;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

