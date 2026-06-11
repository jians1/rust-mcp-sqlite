use serde_json::json;
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::response::StatementResult;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};

fn temp_db_path(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    (dir, path)
}

async fn executor(path: std::path::PathBuf, mode: RunMode, max_rows: usize) -> SqliteExecutor {
    SqliteExecutor::open(ExecutorConfig {
        db_path: path,
        mode,
        max_rows,
        timeout_ms: 10_000,
    })
    .unwrap()
}

#[tokio::test]
async fn basic_select_maps_sqlite_types() {
    let (_dir, path) = temp_db_path("types.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec
        .execute("SELECT NULL AS n, 7 AS i, 1.5 AS r, 'text' AS t, x'6869' AS b".to_string())
        .await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 1);
    let StatementResult::Query(result) = &response.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(result.statement_index, 0);
    assert_eq!(result.statement_type, "SELECT");
    assert_eq!(result.columns, vec!["n", "i", "r", "t", "b"]);
    assert_eq!(result.row_count, 1);
    assert!(!result.truncated);
    assert_eq!(
        result.rows[0],
        serde_json::Map::from_iter([
            ("n".to_string(), serde_json::Value::Null),
            ("i".to_string(), json!(7)),
            ("r".to_string(), json!(1.5)),
            ("t".to_string(), json!("text")),
            (
                "b".to_string(),
                json!({"type": "blob", "encoding": "base64", "data": "aGk="}),
            ),
        ])
    );
}

#[tokio::test]
async fn schema_and_insert_and_update_have_expected_shapes() {
    let (_dir, path) = temp_db_path("write.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec
        .execute(
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users(name) VALUES ('foo');
             UPDATE users SET name = 'bar' WHERE id = 1;"
                .to_string(),
        )
        .await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 3);
    assert!(matches!(response.results[0], StatementResult::Schema(_)));
    let StatementResult::Insert(insert) = &response.results[1] else {
        panic!("expected insert result");
    };
    assert_eq!(insert.affected_rows, 1);
    assert_eq!(insert.last_insert_rowid, 1);
    let StatementResult::Affected(update) = &response.results[2] else {
        panic!("expected affected result");
    };
    assert_eq!(update.statement_type, "UPDATE");
    assert_eq!(update.affected_rows, 1);
}

#[tokio::test]
async fn transaction_rolls_back_all_statements_on_error() {
    let (_dir, path) = temp_db_path("rollback.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let create = exec
        .execute("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT UNIQUE);".to_string())
        .await;
    assert!(create.success, "{create:?}");

    let failed = exec
        .execute(
            "INSERT INTO users(name) VALUES ('foo');
             INSERT INTO users(name) VALUES ('foo');"
                .to_string(),
        )
        .await;
    assert!(!failed.success);
    assert_eq!(failed.results.len(), 0);
    assert_eq!(failed.error.as_ref().unwrap().statement_index, 1);

    let check = exec.execute("SELECT COUNT(*) AS c FROM users;".to_string()).await;
    let StatementResult::Query(result) = &check.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(result.rows[0]["c"], json!(0));
}

#[tokio::test]
async fn multiple_selects_return_multiple_results() {
    let (_dir, path) = temp_db_path("multi_select.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    let response = exec.execute("SELECT 1 AS a; SELECT 2 AS b;".to_string()).await;

    assert!(response.success, "{response:?}");
    assert_eq!(response.results.len(), 2);
    let StatementResult::Query(first) = &response.results[0] else {
        panic!("expected query result");
    };
    let StatementResult::Query(second) = &response.results[1] else {
        panic!("expected query result");
    };
    assert_eq!(first.rows[0]["a"], json!(1));
    assert_eq!(second.rows[0]["b"], json!(2));
}

#[tokio::test]
async fn explicit_transaction_control_is_rejected() {
    let (_dir, path) = temp_db_path("transaction_control.db");
    let exec = executor(path, RunMode::Readwrite, 500).await;

    for sql in ["BEGIN", "COMMIT", "ROLLBACK", "SAVEPOINT x", "RELEASE x"] {
        let response = exec.execute(sql.to_string()).await;
        assert!(!response.success, "{sql} should fail");
        assert_eq!(response.results.len(), 0);
        assert!(
            response
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("transaction control statements are not allowed"),
            "{response:?}"
        );
    }
}
