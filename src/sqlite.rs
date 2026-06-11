use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use fallible_iterator::FallibleIterator;
use rusqlite::{
    Batch, Connection, OpenFlags, Row, Statement,
    types::ValueRef,
};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::RunMode,
    error::AppError,
    response::{
        AffectedResult, ExecuteSqlResponse, InsertResult, QueryResult, SchemaResult, SqlErrorBody,
        StatementResult, SuccessResult,
    },
    sql_classify::{StatementKind, classify, public_statement_type},
};

#[derive(Clone)]
pub struct SqliteExecutor {
    tx: mpsc::Sender<ExecuteJob>,
}

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub db_path: PathBuf,
    pub mode: RunMode,
    pub max_rows: usize,
    pub timeout_ms: u64,
}

struct ExecuteJob {
    sql: String,
    reply: oneshot::Sender<ExecuteSqlResponse>,
}

struct ExecuteFailure {
    message: String,
    statement_index: usize,
}

impl ExecuteFailure {
    fn new(message: impl Into<String>, statement_index: usize) -> Self {
        Self {
            message: message.into(),
            statement_index,
        }
    }
}

impl SqliteExecutor {
    pub fn open(config: ExecutorConfig) -> Result<Self, AppError> {
        let flags = match config.mode {
            RunMode::Readonly => OpenFlags::SQLITE_OPEN_READ_ONLY,
            RunMode::Readwrite => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        };
        let conn = Connection::open_with_flags(&config.db_path, flags)?;
        let (tx, mut rx) = mpsc::channel::<ExecuteJob>(32);

        thread::Builder::new()
            .name("sqlite-mcp-rs-worker".to_string())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    let response = execute_job(&conn, &config, job.sql);
                    let _ = job.reply.send(response);
                }
            })?;

        Ok(Self { tx })
    }

    pub async fn execute(&self, sql: String) -> ExecuteSqlResponse {
        let start = Instant::now();
        let (reply, response) = oneshot::channel();

        if self.tx.send(ExecuteJob { sql, reply }).await.is_err() {
            return failure_response("sqlite worker is not available", 0, start.elapsed());
        }

        response.await.unwrap_or_else(|_| {
            failure_response("sqlite worker stopped before returning a response", 0, start.elapsed())
        })
    }
}

fn execute_job(conn: &Connection, config: &ExecutorConfig, sql: String) -> ExecuteSqlResponse {
    let start = Instant::now();

    match execute_on_connection(conn, &sql, config.max_rows) {
        Ok(results) => ExecuteSqlResponse {
            success: true,
            error: None,
            results,
            elapsed_ms: start.elapsed().as_millis(),
        },
        Err(error) => failure_response(
            error.message,
            error.statement_index,
            start.elapsed(),
        ),
    }
}

fn execute_on_connection(
    conn: &Connection,
    sql: &str,
    _max_rows: usize,
) -> Result<Vec<StatementResult>, ExecuteFailure> {
    if sql.trim().is_empty() {
        return Err(ExecuteFailure::new("sql must not be empty", 0));
    }

    conn.execute_batch("BEGIN")
        .map_err(|error| ExecuteFailure::new(error.to_string(), 0))?;

    let result = execute_batch_statements(conn, sql);

    match result {
        Ok(results) => {
            if let Err(error) = conn.execute_batch("COMMIT") {
                let _ = conn.execute_batch("ROLLBACK");
                Err(ExecuteFailure::new(error.to_string(), results.len()))
            } else {
                Ok(results)
            }
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn execute_batch_statements(
    conn: &Connection,
    sql: &str,
) -> Result<Vec<StatementResult>, ExecuteFailure> {
    let mut batch = Batch::new(conn, sql);
    let mut results = Vec::new();
    let mut statement_index = 0;

    loop {
        let statement = batch
            .next()
            .map_err(|error| ExecuteFailure::new(error.to_string(), statement_index))?;
        let Some(mut statement) = statement else {
            break;
        };

        let result = execute_statement(conn, &mut statement, statement_index)?;
        results.push(result);
        statement_index += 1;
    }

    Ok(results)
}

fn execute_statement(
    conn: &Connection,
    statement: &mut Statement<'_>,
    statement_index: usize,
) -> Result<StatementResult, ExecuteFailure> {
    let sql = statement.expanded_sql().unwrap_or_default();
    let kind = classify(&sql);

    if kind.is_transaction_control() {
        return Err(ExecuteFailure::new(
            "transaction control statements are not allowed",
            statement_index,
        ));
    }

    if statement.column_count() > 0 {
        collect_query_result(statement, kind, statement_index)
            .map_err(|error| ExecuteFailure::new(error.to_string(), statement_index))
    } else {
        execute_non_query(conn, statement, kind, statement_index)
            .map_err(|error| ExecuteFailure::new(error.to_string(), statement_index))
    }
}

fn collect_query_result(
    statement: &mut Statement<'_>,
    kind: StatementKind,
    statement_index: usize,
) -> rusqlite::Result<StatementResult> {
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = statement.query([])?;
    let mut mapped_rows = Vec::new();

    while let Some(row) = rows.next()? {
        mapped_rows.push(row_to_json(row, &columns)?);
    }

    Ok(StatementResult::Query(QueryResult {
        statement_index,
        statement_type: public_statement_type(kind).to_string(),
        row_count: mapped_rows.len(),
        columns,
        rows: mapped_rows,
        truncated: false,
    }))
}

fn execute_non_query(
    conn: &Connection,
    statement: &mut Statement<'_>,
    kind: StatementKind,
    statement_index: usize,
) -> rusqlite::Result<StatementResult> {
    let affected_rows = statement.execute([])?;
    let statement_type = public_statement_type(kind).to_string();

    let result = match kind {
        StatementKind::Insert | StatementKind::Replace => StatementResult::Insert(InsertResult {
            statement_index,
            statement_type,
            affected_rows,
            last_insert_rowid: conn.last_insert_rowid(),
        }),
        StatementKind::Update | StatementKind::Delete => StatementResult::Affected(AffectedResult {
            statement_index,
            statement_type,
            affected_rows,
        }),
        StatementKind::Create | StatementKind::Drop | StatementKind::Alter => {
            StatementResult::Schema(SchemaResult {
                statement_index,
                statement_type,
                success: true,
                schema_changed: true,
            })
        }
        _ => StatementResult::Success(SuccessResult {
            statement_index,
            statement_type,
            success: true,
        }),
    };

    Ok(result)
}

fn row_to_json(row: &Row<'_>, columns: &[String]) -> rusqlite::Result<Map<String, Value>> {
    let mut object = Map::with_capacity(columns.len());

    for (index, column) in columns.iter().enumerate() {
        object.insert(column.clone(), value_to_json(row.get_ref(index)?));
    }

    Ok(object)
}

fn value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => json!({
            "type": "blob",
            "encoding": "base64",
            "data": STANDARD.encode(value),
        }),
    }
}

fn failure_response(
    message: impl Into<String>,
    statement_index: usize,
    elapsed: Duration,
) -> ExecuteSqlResponse {
    ExecuteSqlResponse {
        success: false,
        error: Some(SqlErrorBody {
            message: message.into(),
            statement_index,
        }),
        results: Vec::new(),
        elapsed_ms: elapsed.as_millis(),
    }
}
