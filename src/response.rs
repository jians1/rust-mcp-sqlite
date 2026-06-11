use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
pub struct ExecuteSqlRequest {
    pub sql: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecuteSqlResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SqlErrorBody>,
    pub results: Vec<StatementResult>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SqlErrorBody {
    pub message: String,
    pub statement_index: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StatementResult {
    Query(QueryResult),
    Insert(InsertResult),
    Affected(AffectedResult),
    Schema(SchemaResult),
    Success(SuccessResult),
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
    pub row_count: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InsertResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub affected_rows: usize,
    pub last_insert_rowid: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AffectedResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub affected_rows: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SchemaResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub success: bool,
    pub schema_changed: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SuccessResult {
    pub statement_index: usize,
    pub statement_type: String,
    pub success: bool,
}
