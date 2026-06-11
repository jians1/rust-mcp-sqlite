use crate::config::RunMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Select,
    Explain,
    With,
    Insert,
    Update,
    Delete,
    Replace,
    Create,
    Drop,
    Alter,
    Pragma,
    Vacuum,
    Analyze,
    Attach,
    Detach,
    Begin,
    Commit,
    Rollback,
    Savepoint,
    Release,
    Other,
}

impl StatementKind {
    pub fn is_transaction_control(self) -> bool {
        matches!(
            self,
            Self::Begin | Self::Commit | Self::Rollback | Self::Savepoint | Self::Release
        )
    }
}

pub fn classify(sql: &str) -> StatementKind {
    let sql = skip_leading_ws_and_comments(sql);
    let Some(keyword) = first_keyword(sql) else {
        return StatementKind::Other;
    };

    match_keyword(keyword).map_or(StatementKind::Other, |kind| {
        if kind == StatementKind::With {
            classify_with_main_statement(sql).unwrap_or(StatementKind::With)
        } else {
            kind
        }
    })
}

pub fn is_forbidden_in_mode(kind: StatementKind, sql: &str, mode: RunMode) -> bool {
    if kind.is_transaction_control() {
        return true;
    }

    if mode == RunMode::Readwrite {
        return false;
    }

    match kind {
        StatementKind::Insert
        | StatementKind::Update
        | StatementKind::Delete
        | StatementKind::Replace
        | StatementKind::Create
        | StatementKind::Drop
        | StatementKind::Alter
        | StatementKind::Vacuum
        | StatementKind::Analyze
        | StatementKind::Attach
        | StatementKind::Detach => true,
        StatementKind::Pragma => contains_unquoted_equals(sql),
        _ => false,
    }
}

pub fn public_statement_type(kind: StatementKind) -> &'static str {
    match kind {
        StatementKind::Select => "SELECT",
        StatementKind::Explain => "EXPLAIN",
        StatementKind::With => "WITH",
        StatementKind::Insert => "INSERT",
        StatementKind::Update => "UPDATE",
        StatementKind::Delete => "DELETE",
        StatementKind::Replace => "REPLACE",
        StatementKind::Create => "CREATE",
        StatementKind::Drop => "DROP",
        StatementKind::Alter => "ALTER",
        StatementKind::Pragma => "PRAGMA",
        StatementKind::Vacuum => "VACUUM",
        StatementKind::Analyze => "ANALYZE",
        StatementKind::Attach => "ATTACH",
        StatementKind::Detach => "DETACH",
        StatementKind::Begin => "BEGIN",
        StatementKind::Commit => "COMMIT",
        StatementKind::Rollback => "ROLLBACK",
        StatementKind::Savepoint => "SAVEPOINT",
        StatementKind::Release => "RELEASE",
        StatementKind::Other => "OTHER",
    }
}

fn skip_leading_ws_and_comments(sql: &str) -> &str {
    let bytes = sql.as_bytes();
    let mut index = 0;

    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        if bytes.get(index..index + 2) == Some(b"--") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }

        if bytes.get(index..index + 2) == Some(b"/*") {
            index += 2;
            while index + 1 < bytes.len() && bytes.get(index..index + 2) != Some(b"*/") {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }

        break;
    }

    &sql[index..]
}

fn first_keyword(sql: &str) -> Option<&str> {
    let bytes = sql.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }

    let mut end = 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphabetic() || bytes[end] == b'_') {
        end += 1;
    }

    Some(&sql[..end])
}

fn match_keyword(keyword: &str) -> Option<StatementKind> {
    if keyword.eq_ignore_ascii_case("SELECT") {
        Some(StatementKind::Select)
    } else if keyword.eq_ignore_ascii_case("EXPLAIN") {
        Some(StatementKind::Explain)
    } else if keyword.eq_ignore_ascii_case("WITH") {
        Some(StatementKind::With)
    } else if keyword.eq_ignore_ascii_case("INSERT") {
        Some(StatementKind::Insert)
    } else if keyword.eq_ignore_ascii_case("UPDATE") {
        Some(StatementKind::Update)
    } else if keyword.eq_ignore_ascii_case("DELETE") {
        Some(StatementKind::Delete)
    } else if keyword.eq_ignore_ascii_case("REPLACE") {
        Some(StatementKind::Replace)
    } else if keyword.eq_ignore_ascii_case("CREATE") {
        Some(StatementKind::Create)
    } else if keyword.eq_ignore_ascii_case("DROP") {
        Some(StatementKind::Drop)
    } else if keyword.eq_ignore_ascii_case("ALTER") {
        Some(StatementKind::Alter)
    } else if keyword.eq_ignore_ascii_case("PRAGMA") {
        Some(StatementKind::Pragma)
    } else if keyword.eq_ignore_ascii_case("VACUUM") {
        Some(StatementKind::Vacuum)
    } else if keyword.eq_ignore_ascii_case("ANALYZE") {
        Some(StatementKind::Analyze)
    } else if keyword.eq_ignore_ascii_case("ATTACH") {
        Some(StatementKind::Attach)
    } else if keyword.eq_ignore_ascii_case("DETACH") {
        Some(StatementKind::Detach)
    } else if keyword.eq_ignore_ascii_case("BEGIN") {
        Some(StatementKind::Begin)
    } else if keyword.eq_ignore_ascii_case("COMMIT") {
        Some(StatementKind::Commit)
    } else if keyword.eq_ignore_ascii_case("ROLLBACK") {
        Some(StatementKind::Rollback)
    } else if keyword.eq_ignore_ascii_case("SAVEPOINT") {
        Some(StatementKind::Savepoint)
    } else if keyword.eq_ignore_ascii_case("RELEASE") {
        Some(StatementKind::Release)
    } else {
        None
    }
}

fn classify_with_main_statement(sql: &str) -> Option<StatementKind> {
    let mut index = first_keyword(sql)?.len();
    let bytes = sql.as_bytes();
    let mut depth = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_quoted(bytes, index, bytes[index]),
            b'[' => index = skip_bracket_identifier(bytes, index),
            b'-' if bytes.get(index..index + 2) == Some(b"--") => {
                index = skip_line_comment(bytes, index);
            }
            b'/' if bytes.get(index..index + 2) == Some(b"/*") => {
                index = skip_block_comment(bytes, index);
            }
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            byte if byte.is_ascii_alphabetic() => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphabetic() || bytes[index] == b'_')
                {
                    index += 1;
                }
                if depth == 0 {
                    let keyword = &sql[start..index];
                    if keyword.eq_ignore_ascii_case("SELECT") {
                        return Some(StatementKind::Select);
                    }
                    if keyword.eq_ignore_ascii_case("INSERT") {
                        return Some(StatementKind::Insert);
                    }
                    if keyword.eq_ignore_ascii_case("UPDATE") {
                        return Some(StatementKind::Update);
                    }
                    if keyword.eq_ignore_ascii_case("DELETE") {
                        return Some(StatementKind::Delete);
                    }
                }
            }
            _ => index += 1,
        }
    }

    None
}

fn contains_unquoted_equals(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => index = skip_quoted(bytes, index, bytes[index]),
            b'[' => index = skip_bracket_identifier(bytes, index),
            b'-' if bytes.get(index..index + 2) == Some(b"--") => {
                index = skip_line_comment(bytes, index);
            }
            b'/' if bytes.get(index..index + 2) == Some(b"/*") => {
                index = skip_block_comment(bytes, index);
            }
            b'=' => return true,
            _ => index += 1,
        }
    }

    false
}

fn skip_quoted(bytes: &[u8], mut index: usize, quote: u8) -> usize {
    index += 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            if bytes.get(index + 1) == Some(&quote) {
                index += 2;
            } else {
                return index + 1;
            }
        } else {
            index += 1;
        }
    }
    bytes.len()
}

fn skip_bracket_identifier(bytes: &[u8], mut index: usize) -> usize {
    index += 1;
    while index < bytes.len() {
        if bytes[index] == b']' {
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    index += 2;
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    index += 2;
    while index + 1 < bytes.len() {
        if bytes.get(index..index + 2) == Some(b"*/") {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}
