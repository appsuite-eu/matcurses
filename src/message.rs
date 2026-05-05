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
}

impl ThreadReply {
    pub fn text(time: &str, author: &str, body: &str) -> Self {
        Self {
            time: time.into(),
            author: author.into(),
            blocks: vec![Block::Text(body.into())],
        }
    }
}

pub struct Message {
    pub time: String,
    pub author: String,
    pub blocks: Vec<Block>,
    pub replies: Vec<ThreadReply>,
    pub reactions: Vec<Reaction>,
}

impl Message {
    pub fn text(time: &str, author: &str, body: &str) -> Self {
        Self {
            time: time.into(),
            author: author.into(),
            blocks: vec![Block::Text(body.into())],
            replies: Vec::new(),
            reactions: Vec::new(),
        }
    }

    pub fn with_code(time: &str, author: &str, prose: &str, code: &str) -> Self {
        let mut blocks = Vec::new();
        if !prose.is_empty() {
            blocks.push(Block::Text(prose.into()));
        }
        blocks.push(Block::Code(code.into()));
        Self {
            time: time.into(),
            author: author.into(),
            blocks,
            replies: Vec::new(),
            reactions: Vec::new(),
        }
    }

    pub fn voice(time: &str, author: &str, duration_secs: u32) -> Self {
        Self {
            time: time.into(),
            author: author.into(),
            blocks: vec![Block::Voice { duration_secs }],
            replies: Vec::new(),
            reactions: Vec::new(),
        }
    }

    pub fn with_replies(mut self, replies: Vec<ThreadReply>) -> Self {
        self.replies = replies;
        self
    }

    #[allow(dead_code)]
    pub fn with_reactions(mut self, reactions: Vec<Reaction>) -> Self {
        self.reactions = reactions;
        self
    }
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

pub fn mock_messages() -> Vec<Message> {
    vec![
        Message::text("09:01", "alice", "salut tout le monde"),
        Message::text("09:01", "bob", "yo"),
        Message::text(
            "09:02",
            "alice",
            "qq1 a une idee pour le bug de hier soir sur la prod ? je tourne en rond depuis ce matin",
        )
        .with_replies(vec![
            ThreadReply::text("09:02", "bob", "tu as la stack trace ?"),
            ThreadReply::text(
                "09:03",
                "alice",
                "non juste un timeout cote loadbalancer, rien de plus",
            ),
            ThreadReply::text(
                "09:04",
                "bob",
                "ok je regarde si on a active le tracing distribue sur cette route",
            ),
        ]),
        Message::text(
            "09:03",
            "carol",
            "regarde les logs nginx, j'ai vu passer plein de 502 vers 22h",
        ),
        Message::text(
            "09:03",
            "alice",
            "oui j'ai vu, mais pas encore correle avec un deploy",
        ),
        Message::text(
            "09:04",
            "bob",
            "il y a eu un push sur main vers 21:45 d'apres git log",
        ),
        Message::text("09:05", "alice", "ah ok, je regarde"),
        Message::text("09:06", "dave", "bonjour"),
        Message::text("09:06", "carol", "salut dave"),
        Message::text(
            "09:07",
            "alice",
            "trouve. c'est la migration 042 qui a change le type de la colonne user_id en bigint sans backfill, du coup les anciennes lignes pointent dans le vide pour le foreign key",
        ),
        Message::with_code(
            "09:08",
            "alice",
            "voici le diff incrimine:",
            "ALTER TABLE orders\n  ALTER COLUMN user_id TYPE bigint\n  USING user_id::bigint;",
        )
        .with_replies(vec![
            ThreadReply::text(
                "09:09",
                "carol",
                "qui a review cette PR sans demander de backfill ?",
            ),
            ThreadReply::text("09:09", "bob", "moi, mea culpa, j'ai pas vu le risque"),
            ThreadReply::text(
                "09:10",
                "carol",
                "pas grave, on en parlera dans le postmortem",
            ),
        ]),
        Message::text("09:08", "bob", "aie"),
        Message::text("09:08", "carol", "rollback ?"),
        Message::text(
            "09:09",
            "alice",
            "non on a deja des nouvelles ecritures dessus, faut faire un fix forward",
        ),
        Message::text("09:10", "dave", "je peux aider ? je connais bien cette table"),
        Message::text("09:11", "alice", "yes, peux-tu ecrire le script de backfill ?"),
        Message::text("09:11", "dave", "ok je m'y mets"),
        Message::with_code(
            "09:12",
            "dave",
            "premier jet pour le backfill:",
            "BEGIN;\nUPDATE orders o\n  SET user_id = u.id\n  FROM users u\n  WHERE u.legacy_id::bigint = o.user_id\n    AND o.user_id IS NOT NULL;\nCOMMIT;",
        ),
        Message::text("09:15", "bob", "lol"),
        Message::text(
            "09:15",
            "bob",
            "je viens de voir que le pre-commit hook etait desactive pour cette PR. on a un linter qui aurait flag le probleme",
        ),
        Message::text("09:16", "carol", "qui a desactive le hook"),
        Message::text(
            "09:16",
            "bob",
            "git blame dit que c'est moi il y a 3 mois pour debugger un truc, oups",
        )
        .with_replies(vec![
            ThreadReply::text("09:17", "alice", "tu peux le reactiver maintenant ?"),
            ThreadReply::text("09:17", "bob", "yes, je push le revert"),
        ]),
        Message::text(
            "09:17",
            "alice",
            "ok ticket pour reactiver et un autre pour audit des hooks",
        ),
        Message::text("09:30", "dave", "script pret, je le passe en review"),
        Message::text("09:31", "alice", "merci, je regarde de suite"),
        Message::text("09:45", "alice", "LGTM, deploy quand tu veux"),
        Message::text("09:46", "dave", "deploy en cours"),
        Message::text("09:50", "dave", "deploy ok, backfill en cours, ETA 10 min"),
        Message::text(
            "10:00",
            "dave",
            "backfill termine, plus aucune ligne orpheline",
        ),
        Message::text("10:01", "carol", "nice"),
        Message::text("10:01", "bob", "merci dave"),
        Message::text(
            "10:02",
            "alice",
            "je fais un postmortem cet aprem, je vous mets un doc en lien",
        ),
        Message::text(
            "10:03",
            "alice",
            "et au passage, on devrait peut-etre ajouter une CI qui refuse les migrations qui changent le type d'une colonne sans script de backfill associe",
        ),
        Message::with_code(
            "10:04",
            "carol",
            "+1, je peux ecrire la regle. squelette en pseudo-rust:",
            "fn check_migration(sql: &str) -> Result<()> {\n    if sql.contains(\"ALTER COLUMN\")\n        && sql.contains(\"TYPE\")\n        && !has_associated_backfill(sql)\n    {\n        bail!(\"type change without backfill\");\n    }\n    Ok(())\n}",
        ),
        Message::text("10:05", "alice", "go"),
    ]
}
