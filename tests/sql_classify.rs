use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::sql_classify::{StatementKind, classify, is_forbidden_in_mode};

#[test]
fn classifies_after_whitespace_and_comments() {
    assert_eq!(classify("  -- comment\n SELECT 1"), StatementKind::Select);
    assert_eq!(
        classify("/* x */\nEXPLAIN QUERY PLAN SELECT 1"),
        StatementKind::Explain
    );
    assert_eq!(classify("insert into t values (1)"), StatementKind::Insert);
    assert_eq!(classify("PrAgMa table_info(users)"), StatementKind::Pragma);
}

#[test]
fn rejects_transaction_control() {
    for sql in ["BEGIN", "commit", "ROLLBACK", "savepoint x", "release x"] {
        let kind = classify(sql);
        assert!(kind.is_transaction_control(), "{sql} should be transaction control");
    }
}

#[test]
fn readonly_rejects_mutating_statements() {
    for sql in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET x = 1",
        "DELETE FROM t",
        "CREATE TABLE t(x)",
        "DROP TABLE t",
        "ALTER TABLE t ADD COLUMN y",
        "VACUUM",
        "ANALYZE",
        "ATTACH DATABASE ':memory:' AS x",
        "DETACH DATABASE x",
        "PRAGMA user_version = 2",
    ] {
        assert!(
            is_forbidden_in_mode(classify(sql), sql, RunMode::Readonly),
            "{sql} should be forbidden in readonly"
        );
    }
}

#[test]
fn readonly_allows_read_statements() {
    for sql in [
        "SELECT 1",
        "EXPLAIN SELECT 1",
        "PRAGMA table_info(users)",
        "WITH cte AS (SELECT 1 AS x) SELECT x FROM cte",
    ] {
        assert!(
            !is_forbidden_in_mode(classify(sql), sql, RunMode::Readonly),
            "{sql} should be allowed in readonly"
        );
    }
}

#[test]
fn classifies_with_main_statement_when_obvious() {
    assert_eq!(
        classify("WITH cte AS (SELECT 1) SELECT * FROM cte"),
        StatementKind::Select
    );
    assert_eq!(
        classify("WITH cte AS (SELECT 1) INSERT INTO t SELECT * FROM cte"),
        StatementKind::Insert
    );
}
