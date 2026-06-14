use std::{
    path::PathBuf,
    sync::Once,
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use fallible_iterator::FallibleIterator;
use rusqlite::{
    Batch, Connection, OpenFlags, Row, Statement,
    ffi::{ErrorCode, sqlite3_auto_extension},
    types::ValueRef,
};
use serde_json::{Map, Value, json};
use sqlite_vec::sqlite3_vec_init;
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::RunMode,
    error::AppError,
    response::{
        AffectedResult, ExecuteSqlResponse, InsertResult, QueryResult, SchemaResult, SqlErrorBody,
        StatementResult, SuccessResult,
    },
    sql_classify::{StatementKind, classify, is_forbidden_in_mode, public_statement_type},
    vector::{
        CreateTextCollectionStorageInput, CreateVectorCollectionInput, DeleteTextsInput,
        DeleteVectorsInput, DescribeTextCollectionInput, DropTextCollectionInput,
        DropVectorCollectionInput, SearchGeneratedTextInput, SearchVectorsInput,
        UpsertGeneratedTextsInput, UpsertVectorsInput, VectorOperation, VectorToolResponse,
        create_text_storage_input, execute_vector_operation, search_generated_text_input,
        upsert_generated_texts_input,
    },
};

#[derive(Clone)]
pub struct SqliteExecutor {
    tx: mpsc::Sender<WorkerJob>,
    mode: RunMode,
}

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub db_path: PathBuf,
    pub mode: RunMode,
    pub max_rows: usize,
    pub max_top_k: usize,
    pub timeout_ms: u64,
}

enum WorkerJob {
    Execute(ExecuteJob),
    Vector(VectorJob),
}

struct ExecuteJob {
    sql: String,
    reply: oneshot::Sender<ExecuteSqlResponse>,
}

struct VectorJob {
    operation: VectorOperation,
    reply: oneshot::Sender<VectorToolResponse>,
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
        register_sqlite_vec();
        let conn = Connection::open_with_flags(&config.db_path, flags)?;
        check_fts5(&conn)?;
        check_sqlite_vec(&conn)?;
        let mode = config.mode;
        let (tx, mut rx) = mpsc::channel::<WorkerJob>(32);

        thread::Builder::new()
            .name("sqlite-mcp-rs-worker".to_string())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    match job {
                        WorkerJob::Execute(job) => {
                            let response = execute_job(&conn, &config, job.sql);
                            let _ = job.reply.send(response);
                        }
                        WorkerJob::Vector(job) => {
                            let response = execute_vector_job(&conn, &config, job.operation);
                            let _ = job.reply.send(response);
                        }
                    }
                }
            })?;

        Ok(Self {
            tx,
            mode,
        })
    }

    pub fn mode(&self) -> RunMode {
        self.mode
    }

    pub async fn execute(&self, sql: String) -> ExecuteSqlResponse {
        let start = Instant::now();
        let (reply, response) = oneshot::channel();

        if self
            .tx
            .send(WorkerJob::Execute(ExecuteJob { sql, reply }))
            .await
            .is_err()
        {
            return failure_response("sqlite worker is not available", 0, start.elapsed());
        }

        response.await.unwrap_or_else(|_| {
            failure_response(
                "sqlite worker stopped before returning a response",
                0,
                start.elapsed(),
            )
        })
    }

    pub async fn create_vector_collection(
        &self,
        input: CreateVectorCollectionInput,
    ) -> VectorToolResponse {
        self.execute_vector(VectorOperation::CreateCollection(input))
            .await
    }

    pub async fn upsert_vectors(&self, input: UpsertVectorsInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::UpsertVectors(input))
            .await
    }

    pub async fn search_vectors(&self, input: SearchVectorsInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::SearchVectors(input))
            .await
    }

    pub async fn delete_vectors(&self, input: DeleteVectorsInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::DeleteVectors(input))
            .await
    }

    pub async fn drop_vector_collection(
        &self,
        input: DropVectorCollectionInput,
    ) -> VectorToolResponse {
        self.execute_vector(VectorOperation::DropCollection(input))
            .await
    }

    pub async fn describe_text_collection(
        &self,
        input: DescribeTextCollectionInput,
    ) -> VectorToolResponse {
        self.execute_vector(VectorOperation::DescribeCollection(input))
            .await
    }

    pub async fn create_text_collection_with_dimension(
        &self,
        input: CreateTextCollectionStorageInput,
    ) -> VectorToolResponse {
        self.execute_vector(VectorOperation::CreateCollection(
            create_text_storage_input(input),
        ))
        .await
    }

    pub async fn upsert_generated_texts(&self, input: UpsertGeneratedTextsInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::UpsertVectors(upsert_generated_texts_input(
            input,
        )))
        .await
    }

    pub async fn search_generated_text(&self, input: SearchGeneratedTextInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::SearchVectors(search_generated_text_input(
            input,
        )))
        .await
    }

    pub async fn delete_texts(&self, input: DeleteTextsInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::DeleteVectors(DeleteVectorsInput {
            collection: input.collection,
            ids: input.ids,
        }))
        .await
    }

    pub async fn drop_text_collection(&self, input: DropTextCollectionInput) -> VectorToolResponse {
        self.execute_vector(VectorOperation::DropCollection(
            DropVectorCollectionInput {
                collection: input.collection,
            },
        ))
        .await
    }

    async fn execute_vector(&self, operation: VectorOperation) -> VectorToolResponse {
        let start = Instant::now();
        let (reply, response) = oneshot::channel();

        if self
            .tx
            .send(WorkerJob::Vector(VectorJob { operation, reply }))
            .await
            .is_err()
        {
            return VectorToolResponse::failure(
                "sqlite worker is not available",
                start.elapsed().as_millis(),
            );
        }

        response.await.unwrap_or_else(|_| {
            VectorToolResponse::failure(
                "sqlite worker stopped before returning a response",
                start.elapsed().as_millis(),
            )
        })
    }
}

fn execute_job(conn: &Connection, config: &ExecutorConfig, sql: String) -> ExecuteSqlResponse {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(config.timeout_ms);

    if let Err(error) = conn.progress_handler(1000, Some(move || Instant::now() >= deadline)) {
        return failure_response(error.to_string(), 0, start.elapsed());
    }

    let result = execute_on_connection(conn, &sql, config);
    let _ = clear_progress_handler(conn);

    match result {
        Ok(results) => ExecuteSqlResponse {
            success: true,
            error: None,
            results,
            elapsed_ms: start.elapsed().as_millis(),
        },
        Err(error) => failure_response(error.message, error.statement_index, start.elapsed()),
    }
}

fn execute_on_connection(
    conn: &Connection,
    sql: &str,
    config: &ExecutorConfig,
) -> Result<Vec<StatementResult>, ExecuteFailure> {
    if sql.trim().is_empty() {
        return Err(ExecuteFailure::new("sql must not be empty", 0));
    }

    conn.execute_batch("BEGIN")
        .map_err(|error| ExecuteFailure::new(error.to_string(), 0))?;

    let result = execute_batch_statements(conn, sql, config);

    match result {
        Ok(results) => {
            if let Err(error) = conn.execute_batch("COMMIT") {
                let _ = clear_progress_handler(conn);
                let _ = conn.execute_batch("ROLLBACK");
                Err(sqlite_failure(error, results.len(), config))
            } else {
                Ok(results)
            }
        }
        Err(error) => {
            let _ = clear_progress_handler(conn);
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn execute_vector_job(
    conn: &Connection,
    config: &ExecutorConfig,
    operation: VectorOperation,
) -> VectorToolResponse {
    let start = Instant::now();
    let deadline = start + Duration::from_millis(config.timeout_ms);

    if let Err(error) = conn.progress_handler(1000, Some(move || Instant::now() >= deadline)) {
        return VectorToolResponse::failure(error.to_string(), start.elapsed().as_millis());
    }

    let result = execute_vector_on_connection(conn, config, operation);
    let _ = clear_progress_handler(conn);

    match result {
        Ok(data) => VectorToolResponse::success(data, start.elapsed().as_millis()),
        Err(message) => VectorToolResponse::failure(message, start.elapsed().as_millis()),
    }
}

fn execute_vector_on_connection(
    conn: &Connection,
    config: &ExecutorConfig,
    operation: VectorOperation,
) -> Result<Map<String, Value>, String> {
    conn.execute_batch("BEGIN")
        .map_err(|error| error.to_string())?;

    let result = execute_vector_operation(conn, config.mode, config.max_top_k, operation);

    match result {
        Ok(data) => {
            if let Err(error) = conn.execute_batch("COMMIT") {
                let _ = clear_progress_handler(conn);
                let _ = conn.execute_batch("ROLLBACK");
                Err(error.to_string())
            } else {
                Ok(data)
            }
        }
        Err(error) => {
            let _ = clear_progress_handler(conn);
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn execute_batch_statements(
    conn: &Connection,
    sql: &str,
    config: &ExecutorConfig,
) -> Result<Vec<StatementResult>, ExecuteFailure> {
    let mut batch = Batch::new(conn, sql);
    let mut results = Vec::new();
    let mut statement_index = 0;

    loop {
        let statement = batch
            .next()
            .map_err(|error| sqlite_failure(error, statement_index, config))?;
        let Some(mut statement) = statement else {
            break;
        };

        let result = execute_statement(conn, &mut statement, statement_index, config)?;
        results.push(result);
        statement_index += 1;
    }

    Ok(results)
}

fn execute_statement(
    conn: &Connection,
    statement: &mut Statement<'_>,
    statement_index: usize,
    config: &ExecutorConfig,
) -> Result<StatementResult, ExecuteFailure> {
    let sql = statement.expanded_sql().unwrap_or_default();
    let kind = classify(&sql);

    if kind.is_transaction_control() {
        return Err(ExecuteFailure::new(
            "transaction control statements are not allowed",
            statement_index,
        ));
    }

    if config.mode == RunMode::Readonly
        && (is_forbidden_in_mode(kind, &sql, RunMode::Readonly) || !statement.readonly())
    {
        return Err(ExecuteFailure::new(
            format!(
                "readonly mode forbids {} statements",
                public_statement_type(kind)
            ),
            statement_index,
        ));
    }

    if statement.column_count() > 0 {
        collect_query_result(statement, kind, statement_index, config.max_rows)
            .map_err(|error| sqlite_failure(error, statement_index, config))
    } else {
        execute_non_query(conn, statement, kind, statement_index)
            .map_err(|error| sqlite_failure(error, statement_index, config))
    }
}

fn collect_query_result(
    statement: &mut Statement<'_>,
    kind: StatementKind,
    statement_index: usize,
    max_rows: usize,
) -> rusqlite::Result<StatementResult> {
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = statement.query([])?;
    let mut mapped_rows = Vec::new();

    while mapped_rows.len() < max_rows {
        let Some(row) = rows.next()? else {
            return Ok(StatementResult::Query(QueryResult {
                statement_index,
                statement_type: public_statement_type(kind).to_string(),
                row_count: mapped_rows.len(),
                columns,
                rows: mapped_rows,
                truncated: false,
            }));
        };
        mapped_rows.push(row_to_json(row, &columns)?);
    }
    let truncated = rows.next()?.is_some();

    Ok(StatementResult::Query(QueryResult {
        statement_index,
        statement_type: public_statement_type(kind).to_string(),
        row_count: mapped_rows.len(),
        columns,
        rows: mapped_rows,
        truncated,
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
        StatementKind::Update | StatementKind::Delete => {
            StatementResult::Affected(AffectedResult {
                statement_index,
                statement_type,
                affected_rows,
            })
        }
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

fn check_fts5(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE temp.__fts5_check USING fts5(x);
         DROP TABLE temp.__fts5_check;",
    )
    .map_err(AppError::from)
}

fn register_sqlite_vec() {
    static REGISTER: Once = Once::new();

    REGISTER.call_once(|| unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    });
}

fn check_sqlite_vec(conn: &Connection) -> Result<(), AppError> {
    conn.query_row("SELECT vec_version()", [], |_row| Ok(()))?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE temp.__vec_check USING vec0(
           embedding float[2] distance_metric=cosine
         );
         DROP TABLE temp.__vec_check;",
    )
    .map_err(AppError::from)
}

fn clear_progress_handler(conn: &Connection) -> rusqlite::Result<()> {
    conn.progress_handler(0, None::<fn() -> bool>)
}

fn sqlite_failure(
    error: rusqlite::Error,
    statement_index: usize,
    config: &ExecutorConfig,
) -> ExecuteFailure {
    if error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) {
        ExecuteFailure::new(
            format!("query timed out after {} ms", config.timeout_ms),
            statement_index,
        )
    } else {
        ExecuteFailure::new(error.to_string(), statement_index)
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
