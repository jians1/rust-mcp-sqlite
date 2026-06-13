use std::convert::TryFrom;

use rusqlite::{Connection, Error, OptionalExtension, params, params_from_iter, types::ToSql};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::RunMode;

const DISTANCE_METRIC: &str = "cosine";

#[derive(Clone, Debug)]
pub enum VectorOperation {
    CreateCollection(CreateVectorCollectionInput),
    UpsertVectors(UpsertVectorsInput),
    SearchVectors(SearchVectorsInput),
    DeleteVectors(DeleteVectorsInput),
    DropCollection(DropVectorCollectionInput),
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct CreateVectorCollectionInput {
    pub collection: String,
    pub dimension: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct UpsertVectorsInput {
    pub collection: String,
    pub items: Vec<VectorItemInput>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct VectorItemInput {
    pub id: String,
    pub vector: Vec<f64>,
    pub text: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct SearchVectorsInput {
    pub collection: String,
    pub vector: Vec<f64>,
    pub top_k: usize,
    pub filter: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct DeleteVectorsInput {
    pub collection: String,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
pub struct DropVectorCollectionInput {
    pub collection: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct VectorToolResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<VectorErrorBody>,
    #[serde(flatten)]
    pub data: Map<String, Value>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct VectorErrorBody {
    pub message: String,
}

impl VectorToolResponse {
    pub fn success(data: Map<String, Value>, elapsed_ms: u128) -> Self {
        Self {
            success: true,
            error: None,
            data,
            elapsed_ms,
        }
    }

    pub fn failure(message: impl Into<String>, elapsed_ms: u128) -> Self {
        Self {
            success: false,
            error: Some(VectorErrorBody {
                message: message.into(),
            }),
            data: Map::new(),
            elapsed_ms,
        }
    }
}

pub fn execute_vector_operation(
    conn: &Connection,
    mode: RunMode,
    max_top_k: usize,
    operation: VectorOperation,
) -> Result<Map<String, Value>, String> {
    match operation {
        VectorOperation::CreateCollection(input) => create_collection(conn, mode, input),
        VectorOperation::UpsertVectors(input) => upsert_vectors(conn, mode, input),
        VectorOperation::SearchVectors(input) => search_vectors(conn, max_top_k, input),
        VectorOperation::DeleteVectors(input) => delete_vectors(conn, mode, input),
        VectorOperation::DropCollection(input) => drop_collection(conn, mode, input),
    }
}

fn create_collection(
    conn: &Connection,
    mode: RunMode,
    input: CreateVectorCollectionInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids create_vector_collection".to_string());
    }

    let collection = validate_collection_name(&input.collection)?;
    if input.dimension == 0 {
        return Err("dimension must be positive".to_string());
    }

    ensure_registry(conn)?;

    if let Some(existing) = find_collection(conn, collection)? {
        if existing.dimension != input.dimension || existing.distance_metric != DISTANCE_METRIC {
            return Err(format!(
                "collection already exists with dimension {} and distance metric {}",
                existing.dimension, existing.distance_metric
            ));
        }
        return Ok(collection_response(
            collection,
            &existing.table_name,
            existing.dimension,
            false,
        ));
    }

    let table_name = collection_table_name(collection);
    create_vec0_table(conn, &table_name, input.dimension)?;
    conn.execute(
        "INSERT INTO __vector_collections(name, table_name, dimension, distance_metric, created_at)
         VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            collection,
            table_name,
            input.dimension as i64,
            DISTANCE_METRIC
        ],
    )
    .map_err(|error| error.to_string())?;

    Ok(collection_response(
        collection,
        &table_name,
        input.dimension,
        true,
    ))
}

fn upsert_vectors(
    conn: &Connection,
    mode: RunMode,
    input: UpsertVectorsInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids upsert_vectors".to_string());
    }

    let collection = validate_collection_name(&input.collection)?;
    ensure_registry(conn)?;
    let existing = find_collection(conn, collection)?
        .ok_or_else(|| format!("collection not found: {collection}"))?;
    let sql = format!(
        "INSERT INTO {}(id, embedding, text, metadata)
         VALUES (?1, vec_f32(?2), ?3, ?4)",
        existing.table_name
    );
    let delete_sql = format!("DELETE FROM {} WHERE id = ?1", existing.table_name);

    for item in &input.items {
        if item.id.is_empty() {
            return Err("vector id must not be empty".to_string());
        }
        let vector_json = vector_to_json(&item.vector, existing.dimension)?;
        let metadata_json = metadata_to_json(item.metadata.as_ref())?;
        conn.execute(&delete_sql, params![item.id])
            .map_err(|error| error.to_string())?;
        conn.execute(
            &sql,
            params![item.id, vector_json, item.text, metadata_json],
        )
        .map_err(|error| error.to_string())?;
    }

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("upserted_count".to_string(), json!(input.items.len())),
    ]))
}

fn search_vectors(
    conn: &Connection,
    max_top_k: usize,
    input: SearchVectorsInput,
) -> Result<Map<String, Value>, String> {
    let collection = validate_collection_name(&input.collection)?;
    let existing = find_collection(conn, collection)?
        .ok_or_else(|| format!("collection not found: {collection}"))?;
    if input.top_k == 0 {
        return Err("top_k must be positive".to_string());
    }
    if input.top_k > max_top_k {
        return Err(format!("top_k must not exceed max_top_k ({max_top_k})"));
    }
    let vector_json = vector_to_json(&input.vector, existing.dimension)?;

    if let Some(filter) = filter_to_map(input.filter.as_ref())? {
        return search_vectors_filtered(
            conn,
            collection,
            &existing.table_name,
            vector_json,
            input.top_k,
            filter,
        );
    }

    let sql = format!(
        "SELECT id, distance, text, metadata
         FROM {}
         WHERE embedding MATCH vec_f32(?1)
         ORDER BY distance
         LIMIT ?2",
        existing.table_name
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![vector_json, input.top_k as i64], |row| {
            let metadata_text: Option<String> = row.get(3)?;
            let metadata = parse_metadata_text(metadata_text.as_deref());
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "distance": row.get::<_, f64>(1)?,
                "text": row.get::<_, Option<String>>(2)?,
                "metadata": metadata,
            }))
        })
        .map_err(|error| error.to_string())?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|error| error.to_string())?);
    }

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("results".to_string(), Value::Array(results)),
    ]))
}

fn search_vectors_filtered(
    conn: &Connection,
    collection: &str,
    table_name: &str,
    vector_json: String,
    top_k: usize,
    filter: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut clauses = Vec::new();
    let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(vector_json)];

    for (key, value) in filter {
        validate_filter_key(key)?;
        let path = format!("$.{key}");
        match value {
            Value::String(value) => {
                clauses.push(format!(
                    "json_type(metadata, '{path}') = 'text' AND json_extract(metadata, '{path}') = ?"
                ));
                values.push(Box::new(value.clone()));
            }
            Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'integer' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                } else if let Some(value) = value.as_u64() {
                    let value = i64::try_from(value)
                        .map_err(|_| "filter integer is too large".to_string())?;
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'integer' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                } else if let Some(value) = value.as_f64() {
                    clauses.push(format!(
                        "json_type(metadata, '{path}') = 'real' AND json_extract(metadata, '{path}') = ?"
                    ));
                    values.push(Box::new(value));
                }
            }
            Value::Bool(value) => {
                let json_type = if *value { "true" } else { "false" };
                clauses.push(format!("json_type(metadata, '{path}') = '{json_type}'"));
            }
            Value::Null => {
                clauses.push(format!("json_type(metadata, '{path}') = 'null'"));
            }
            Value::Array(_) | Value::Object(_) => {
                return Err("filter values must be scalar JSON values".to_string());
            }
        }
    }

    values.push(Box::new(top_k as i64));
    let where_clause = if clauses.is_empty() {
        "1".to_string()
    } else {
        clauses.join(" AND ")
    };
    let sql = format!(
        "SELECT id, vec_distance_cosine(embedding, vec_f32(?)) AS distance, text, metadata
         FROM {table_name}
         WHERE {where_clause}
         ORDER BY distance
         LIMIT ?"
    );
    let params = params_from_iter(values.iter().map(|value| value.as_ref() as &dyn ToSql));
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params, |row| {
            let metadata_text: Option<String> = row.get(3)?;
            let metadata = parse_metadata_text(metadata_text.as_deref());
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "distance": row.get::<_, f64>(1)?,
                "text": row.get::<_, Option<String>>(2)?,
                "metadata": metadata,
            }))
        })
        .map_err(|error| error.to_string())?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|error| error.to_string())?);
    }

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("results".to_string(), Value::Array(results)),
    ]))
}

fn delete_vectors(
    conn: &Connection,
    mode: RunMode,
    input: DeleteVectorsInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids delete_vectors".to_string());
    }

    let collection = validate_collection_name(&input.collection)?;
    ensure_registry(conn)?;
    let existing = find_collection(conn, collection)?
        .ok_or_else(|| format!("collection not found: {collection}"))?;
    let sql = format!("DELETE FROM {} WHERE id = ?1", existing.table_name);
    let mut deleted_count = 0usize;

    for id in &input.ids {
        if id.is_empty() {
            return Err("vector id must not be empty".to_string());
        }
        deleted_count += conn
            .execute(&sql, params![id])
            .map_err(|error| error.to_string())?;
    }

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("requested_count".to_string(), json!(input.ids.len())),
        ("deleted_count".to_string(), json!(deleted_count)),
    ]))
}

fn drop_collection(
    conn: &Connection,
    mode: RunMode,
    input: DropVectorCollectionInput,
) -> Result<Map<String, Value>, String> {
    if mode == RunMode::Readonly {
        return Err("readonly mode forbids drop_vector_collection".to_string());
    }

    let collection = validate_collection_name(&input.collection)?;
    ensure_registry(conn)?;
    let Some(existing) = find_collection(conn, collection)? else {
        return Ok(Map::from_iter([
            ("collection".to_string(), json!(collection)),
            ("existed".to_string(), json!(false)),
        ]));
    };

    let sql = format!("DROP TABLE IF EXISTS {}", existing.table_name);
    conn.execute_batch(&sql)
        .map_err(|error| error.to_string())?;
    conn.execute(
        "DELETE FROM __vector_collections WHERE name = ?1",
        params![collection],
    )
    .map_err(|error| error.to_string())?;

    Ok(Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("existed".to_string(), json!(true)),
    ]))
}

fn parse_metadata_text(metadata: Option<&str>) -> Value {
    metadata
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| json!({}))
}

fn vector_to_json(vector: &[f64], expected_dimension: usize) -> Result<String, String> {
    if vector.len() != expected_dimension {
        return Err(format!(
            "vector dimension mismatch: expected {}, got {}",
            expected_dimension,
            vector.len()
        ));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err("vector contains a non-finite value".to_string());
    }

    serde_json::to_string(vector).map_err(|error| error.to_string())
}

fn metadata_to_json(metadata: Option<&Value>) -> Result<String, String> {
    let Some(metadata) = metadata else {
        return Ok("{}".to_string());
    };
    if !metadata.is_object() {
        return Err("metadata must be a JSON object".to_string());
    }

    serde_json::to_string(metadata).map_err(|error| error.to_string())
}

fn filter_to_map(filter: Option<&Value>) -> Result<Option<&Map<String, Value>>, String> {
    match filter {
        None => Ok(None),
        Some(Value::Object(filter)) => Ok(Some(filter)),
        Some(_) => Err("filter must be a JSON object".to_string()),
    }
}

fn validate_collection_name(name: &str) -> Result<&str, String> {
    if name.is_empty() {
        return Err("collection name must not be empty".to_string());
    }
    if name.starts_with("__") {
        return Err("collection name must not start with __".to_string());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(
            "collection name must contain only letters, digits, and underscores".to_string(),
        );
    }

    Ok(name)
}

fn validate_filter_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("filter keys must not be empty".to_string());
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("filter keys must contain only letters, digits, and underscores".to_string());
    }

    Ok(())
}

fn ensure_registry(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS __vector_collections (
           name TEXT PRIMARY KEY,
           table_name TEXT NOT NULL UNIQUE,
           dimension INTEGER NOT NULL,
           distance_metric TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )
    .map_err(|error| error.to_string())
}

struct ExistingCollection {
    table_name: String,
    dimension: usize,
    distance_metric: String,
}

fn find_collection(
    conn: &Connection,
    collection: &str,
) -> Result<Option<ExistingCollection>, String> {
    let result = conn
        .query_row(
            "SELECT table_name, dimension, distance_metric
         FROM __vector_collections
         WHERE name = ?1",
            [collection],
            |row| {
                let dimension = row.get::<_, i64>(1)?;
                Ok(ExistingCollection {
                    table_name: row.get(0)?,
                    dimension: usize::try_from(dimension).unwrap_or(0),
                    distance_metric: row.get(2)?,
                })
            },
        )
        .optional();

    match result {
        Ok(collection) => Ok(collection),
        Err(error) if is_missing_registry_error(&error) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn is_missing_registry_error(error: &Error) -> bool {
    matches!(
        error,
        Error::SqliteFailure(_, Some(message))
            if message.contains("no such table: __vector_collections")
    )
}

fn collection_table_name(collection: &str) -> String {
    format!("vec_{collection}")
}

fn create_vec0_table(conn: &Connection, table_name: &str, dimension: usize) -> Result<(), String> {
    let sql = format!(
        "CREATE VIRTUAL TABLE {table_name} USING vec0(
           id TEXT PRIMARY KEY,
           embedding float[{dimension}] distance_metric=cosine,
           +text TEXT,
           +metadata TEXT
         );"
    );
    conn.execute_batch(&sql).map_err(|error| error.to_string())
}

fn collection_response(
    collection: &str,
    table_name: &str,
    dimension: usize,
    created: bool,
) -> Map<String, Value> {
    Map::from_iter([
        ("collection".to_string(), json!(collection)),
        ("table_name".to_string(), json!(table_name)),
        ("dimension".to_string(), json!(dimension)),
        ("distance_metric".to_string(), json!(DISTANCE_METRIC)),
        ("created".to_string(), json!(created)),
    ])
}
