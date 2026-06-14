use serde_json::json;
use sqlite_mcp_rs::config::RunMode;
use sqlite_mcp_rs::response::StatementResult;
use sqlite_mcp_rs::sqlite::{ExecutorConfig, SqliteExecutor};
use sqlite_mcp_rs::vector::{
    CreateTextCollectionStorageInput, DeleteTextsInput, DropTextCollectionInput,
    GeneratedTextItemInput, SearchGeneratedHybridTextInput, SearchGeneratedTextInput,
    UpsertGeneratedTextsInput,
};

fn temp_db_path(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    (dir, path)
}

async fn executor(
    path: std::path::PathBuf,
    mode: RunMode,
    max_rows: usize,
    max_top_k: usize,
) -> SqliteExecutor {
    SqliteExecutor::open(ExecutorConfig {
        db_path: path,
        mode,
        max_rows,
        max_top_k,
        timeout_ms: 10_000,
    })
    .unwrap()
}

#[tokio::test]
async fn generated_text_vectors_store_text_and_search_without_vectors() {
    let (_dir, path) = temp_db_path("generated_text_vectors.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "alpha".to_string(),
                metadata: Some(json!({"tenant": "a"})),
            }],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");
    assert_eq!(upsert.data["upserted_count"], json!(1));

    let search = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
        })
        .await;
    assert!(search.success, "{search:?}");
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("doc-a"));
    assert_eq!(results[0]["text"], json!("alpha"));
    assert_eq!(results[0]["metadata"], json!({"tenant": "a"}));
    assert!(results[0].get("vector").is_none());
}

#[tokio::test]
async fn create_collection_writes_registry() {
    let (_dir, path) = temp_db_path("create_collection.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let created = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;

    assert!(created.success, "{created:?}");
    assert_eq!(created.data["collection"], json!("docs"));
    assert_eq!(created.data["table_name"], json!("vec_docs"));
    assert_eq!(created.data["dimension"], json!(2));
    assert_eq!(created.data["distance_metric"], json!("cosine"));
    assert_eq!(created.data["created"], json!(true));

    let duplicate = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;

    assert!(duplicate.success, "{duplicate:?}");
    assert_eq!(duplicate.data["created"], json!(false));

    let registry = exec
        .execute(
            "SELECT name, table_name, dimension, distance_metric
             FROM __vector_collections
             WHERE name = 'docs';"
                .to_string(),
        )
        .await;

    assert!(registry.success, "{registry:?}");
    let StatementResult::Query(query) = &registry.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 1);
    assert_eq!(query.rows[0]["name"], json!("docs"));
    assert_eq!(query.rows[0]["table_name"], json!("vec_docs"));
    assert_eq!(query.rows[0]["dimension"], json!(2));
    assert_eq!(query.rows[0]["distance_metric"], json!("cosine"));

    let table = exec
        .execute("SELECT id, text, metadata FROM vec_docs;".to_string())
        .await;
    assert!(table.success, "{table:?}");
}

#[tokio::test]
async fn create_collection_adds_fts_and_tag_sidecars() {
    let (_dir, path) = temp_db_path("hybrid_sidecars.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let sidecars = exec
        .execute(
            "SELECT name, type
             FROM sqlite_master
             WHERE name IN ('fts_docs', 'tags_docs')
             ORDER BY name;"
                .to_string(),
        )
        .await;
    assert!(sidecars.success, "{sidecars:?}");
    let StatementResult::Query(query) = &sidecars.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 2);
    assert_eq!(query.rows[0]["name"], json!("fts_docs"));
    assert_eq!(query.rows[1]["name"], json!("tags_docs"));
}

#[tokio::test]
async fn create_collection_is_idempotent_for_same_dimension() {
    let (_dir, path) = temp_db_path("create_collection_idempotent.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let created = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(created.success, "{created:?}");
    assert_eq!(created.data["created"], json!(true));

    let duplicate = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(duplicate.success, "{duplicate:?}");
    assert_eq!(duplicate.data["created"], json!(false));

    let conflict = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 3,
        })
        .await;
    assert!(!conflict.success, "{conflict:?}");
    assert!(vector_error_message(&conflict).contains("already exists"));

    let registry = exec
        .execute("SELECT dimension FROM __vector_collections WHERE name = 'docs';".to_string())
        .await;
    assert!(registry.success, "{registry:?}");
    let StatementResult::Query(query) = &registry.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.rows[0]["dimension"], json!(2));
}

#[tokio::test]
async fn upsert_generated_texts_replaces_existing_records() {
    let (_dir, path) = temp_db_path("upsert_generated_texts.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let first = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-1".to_string(),
                vector: vec![1.0, 0.0],
                text: "first".to_string(),
                metadata: Some(json!({"source": "draft"})),
            }],
        })
        .await;
    assert!(first.success, "{first:?}");
    assert_eq!(first.data["upserted_count"], json!(1));

    let second = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-1".to_string(),
                vector: vec![0.0, 1.0],
                text: "second".to_string(),
                metadata: Some(json!({"source": "final"})),
            }],
        })
        .await;
    assert!(second.success, "{second:?}");
    assert_eq!(second.data["upserted_count"], json!(1));

    let row = exec
        .execute(
            "SELECT id, vec_to_json(embedding) AS embedding, text, metadata
             FROM vec_docs
             WHERE id = 'doc-1';"
                .to_string(),
        )
        .await;
    assert!(row.success, "{row:?}");
    let StatementResult::Query(query) = &row.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 1);
    assert_eq!(query.rows[0]["id"], json!("doc-1"));
    assert_eq!(query.rows[0]["embedding"], json!("[0.000000,1.000000]"));
    assert_eq!(query.rows[0]["text"], json!("second"));
    assert_eq!(query.rows[0]["metadata"], json!(r#"{"source":"final"}"#));
}

#[tokio::test]
async fn upsert_generated_texts_indexes_fts_and_metadata_tags() {
    let (_dir, path) = temp_db_path("hybrid_indexing.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "她没有拔剑，只是抬眼看过去，殿中喧哗便像被霜压住。".to_string(),
                metadata: Some(json!({
                    "tenant": "novel",
                    "tags": ["女主", "克制", "压迫感"]
                })),
            }],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let fts = exec
        .execute(
            "SELECT id
             FROM fts_docs
             WHERE fts_docs MATCH '\"殿中喧哗\"';"
                .to_string(),
        )
        .await;
    assert!(fts.success, "{fts:?}");
    let StatementResult::Query(fts_query) = &fts.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(fts_query.row_count, 1);
    assert_eq!(fts_query.rows[0]["id"], json!("doc-a"));

    let tags = exec
        .execute(
            "SELECT tag
             FROM tags_docs
             WHERE item_id = 'doc-a'
             ORDER BY tag;"
                .to_string(),
        )
        .await;
    assert!(tags.success, "{tags:?}");
    let StatementResult::Query(tags_query) = &tags.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(tags_query.row_count, 3);
    assert_eq!(tags_query.rows[0]["tag"], json!("克制"));
    assert_eq!(tags_query.rows[1]["tag"], json!("压迫感"));
    assert_eq!(tags_query.rows[2]["tag"], json!("女主"));
}

#[tokio::test]
async fn upsert_generated_texts_rolls_back_entire_batch_on_invalid_item() {
    let (_dir, path) = temp_db_path("upsert_generated_texts_rollback.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "doc-a".to_string(),
                    vector: vec![1.0, 0.0],
                    text: "text".to_string(),
                    metadata: None,
                },
                GeneratedTextItemInput {
                    id: "doc-b".to_string(),
                    vector: vec![0.0],
                    text: "text".to_string(),
                    metadata: None,
                },
            ],
        })
        .await;

    assert!(!upsert.success, "{upsert:?}");
    assert!(vector_error_message(&upsert).contains("dimension mismatch"));

    let rows = exec
        .execute("SELECT COUNT(*) AS count FROM vec_docs;".to_string())
        .await;
    assert!(rows.success, "{rows:?}");
    let StatementResult::Query(query) = &rows.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.rows[0]["count"], json!(0));
}

#[tokio::test]
async fn search_generated_text_returns_top_k_without_vectors() {
    let (_dir, path) = temp_db_path("search_generated_text.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "doc-a".to_string(),
                    vector: vec![1.0, 0.0],
                    text: "alpha".to_string(),
                    metadata: Some(json!({"source": "manual"})),
                },
                GeneratedTextItemInput {
                    id: "doc-b".to_string(),
                    vector: vec![0.0, 1.0],
                    text: "beta".to_string(),
                    metadata: Some(json!({"source": "manual"})),
                },
            ],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let search = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
        })
        .await;

    assert!(search.success, "{search:?}");
    assert_eq!(search.data["collection"], json!("docs"));
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("doc-a"));
    assert_eq!(results[0]["text"], json!("alpha"));
    assert_eq!(results[0]["metadata"], json!({"source": "manual"}));
    assert_eq!(results[0].get("vector"), None);
    assert!(results[0]["distance"].as_f64().unwrap() <= 0.000001);
}

#[tokio::test]
async fn validation_rejects_invalid_inputs() {
    let (_dir, path) = temp_db_path("validation.db");
    let exec = executor(path, RunMode::Readwrite, 1, 2).await;

    let invalid_name = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "bad-name".to_string(),
            dimension: 2,
        })
        .await;
    assert!(!invalid_name.success, "{invalid_name:?}");
    assert!(vector_error_message(&invalid_name).contains("letters, digits, and underscores"));

    let reserved_name = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "__internal".to_string(),
            dimension: 2,
        })
        .await;
    assert!(!reserved_name.success, "{reserved_name:?}");
    assert!(vector_error_message(&reserved_name).contains("must not start with __"));

    let zero_dimension = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 0,
        })
        .await;
    assert!(!zero_dimension.success, "{zero_dimension:?}");
    assert!(vector_error_message(&zero_dimension).contains("dimension must be positive"));

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let empty_id = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: String::new(),
                vector: vec![1.0, 0.0],
                text: "text".to_string(),
                metadata: None,
            }],
        })
        .await;
    assert!(!empty_id.success, "{empty_id:?}");
    assert!(vector_error_message(&empty_id).contains("id must not be empty"));

    let dimension_mismatch = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0],
                text: "text".to_string(),
                metadata: None,
            }],
        })
        .await;
    assert!(!dimension_mismatch.success, "{dimension_mismatch:?}");
    assert!(vector_error_message(&dimension_mismatch).contains("dimension mismatch"));

    let non_finite = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![f64::NAN, 0.0],
                text: "text".to_string(),
                metadata: None,
            }],
        })
        .await;
    assert!(!non_finite.success, "{non_finite:?}");
    assert!(vector_error_message(&non_finite).contains("non-finite"));

    let non_object_metadata = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "text".to_string(),
                metadata: Some(json!(["not", "object"])),
            }],
        })
        .await;
    assert!(!non_object_metadata.success, "{non_object_metadata:?}");
    assert!(vector_error_message(&non_object_metadata).contains("metadata must be a JSON object"));

    let top_k_zero = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 0,
            filter: None,
        })
        .await;
    assert!(!top_k_zero.success, "{top_k_zero:?}");
    assert!(vector_error_message(&top_k_zero).contains("top_k must be positive"));

    let top_k_exceeds_max_rows_only = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 2,
            filter: None,
        })
        .await;
    assert!(
        top_k_exceeds_max_rows_only.success,
        "{top_k_exceeds_max_rows_only:?}"
    );

    let top_k_too_large = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 3,
            filter: None,
        })
        .await;
    assert!(!top_k_too_large.success, "{top_k_too_large:?}");
    assert!(vector_error_message(&top_k_too_large).contains("max_top_k"));

    let invalid_filter_key = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: Some(json!({"tenant.name": "a"})),
        })
        .await;
    assert!(!invalid_filter_key.success, "{invalid_filter_key:?}");
    assert!(vector_error_message(&invalid_filter_key).contains("filter keys"));

    let non_object_filter = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: Some(json!(["not", "object"])),
        })
        .await;
    assert!(!non_object_filter.success, "{non_object_filter:?}");
    assert!(vector_error_message(&non_object_filter).contains("filter must be a JSON object"));

    let unsupported_filter_value = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: Some(json!({"tenant": ["a"]})),
        })
        .await;
    assert!(
        !unsupported_filter_value.success,
        "{unsupported_filter_value:?}"
    );
    assert!(vector_error_message(&unsupported_filter_value).contains("scalar JSON values"));
}

#[tokio::test]
async fn search_generated_text_filters_metadata() {
    let (_dir, path) = temp_db_path("search_filter.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "doc-near".to_string(),
                    vector: vec![1.0, 0.0],
                    text: "near but wrong tenant".to_string(),
                    metadata: Some(json!({"tenant": "b", "rank": 1})),
                },
                GeneratedTextItemInput {
                    id: "doc-match".to_string(),
                    vector: vec![0.0, 1.0],
                    text: "matching tenant".to_string(),
                    metadata: Some(json!({"tenant": "a", "rank": 2})),
                },
            ],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let search = exec
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 5,
            filter: Some(json!({"tenant": "a", "rank": 2})),
        })
        .await;

    assert!(search.success, "{search:?}");
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("doc-match"));
    assert_eq!(results[0]["metadata"], json!({"tenant": "a", "rank": 2}));
}

#[tokio::test]
async fn search_generated_text_hybrid_filters_fts_tags_and_metadata_then_sorts_by_vector() {
    let (_dir, path) = temp_db_path("hybrid_search.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 4,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "a1".to_string(),
                    vector: vec![0.98, 0.02, 0.04, 0.10],
                    text: "她没有拔剑，只是抬眼看过去，殿中喧哗便像被霜压住。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "高潮燃点",
                        "score": 9,
                        "tags": ["女主", "克制", "压迫感"]
                    })),
                },
                GeneratedTextItemInput {
                    id: "a2".to_string(),
                    vector: vec![0.90, 0.05, 0.02, 0.02],
                    text: "他站在雨里，眉眼淡得像旧雪，偏让人不敢靠近。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "人物塑造",
                        "score": 9,
                        "tags": ["男主", "克制", "压迫感"]
                    })),
                },
                GeneratedTextItemInput {
                    id: "a3".to_string(),
                    vector: vec![0.04, 0.97, 0.01, 0.02],
                    text: "她把杯子往前一推，笑说这次轮到你认输。".to_string(),
                    metadata: Some(json!({
                        "tenant": "novel",
                        "dimension": "日常互动",
                        "score": 8,
                        "tags": ["女主", "轻松"]
                    })),
                },
            ],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let search = exec
        .search_generated_text_hybrid(SearchGeneratedHybridTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0, 0.0, 0.0],
            top_k: 5,
            filter: Some(json!({"tenant": "novel"})),
            fts_query: Some("殿中喧哗".to_string()),
            tags: vec!["女主".to_string(), "克制".to_string()],
        })
        .await;

    assert!(search.success, "{search:?}");
    let results = search.data["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], json!("a1"));
    assert_eq!(results[0]["metadata"]["dimension"], json!("高潮燃点"));
}

#[tokio::test]
async fn delete_texts_reports_requested_and_deleted_counts() {
    let (_dir, path) = temp_db_path("delete_texts.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![
                GeneratedTextItemInput {
                    id: "doc-a".to_string(),
                    vector: vec![1.0, 0.0],
                    text: "text".to_string(),
                    metadata: None,
                },
                GeneratedTextItemInput {
                    id: "doc-b".to_string(),
                    vector: vec![0.0, 1.0],
                    text: "text".to_string(),
                    metadata: None,
                },
            ],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let deleted = exec
        .delete_texts(DeleteTextsInput {
            collection: "docs".to_string(),
            ids: vec!["doc-a".to_string(), "missing".to_string()],
        })
        .await;

    assert!(deleted.success, "{deleted:?}");
    assert_eq!(deleted.data["collection"], json!("docs"));
    assert_eq!(deleted.data["requested_count"], json!(2));
    assert_eq!(deleted.data["deleted_count"], json!(1));

    let remaining = exec
        .execute("SELECT id FROM vec_docs ORDER BY id;".to_string())
        .await;
    assert!(remaining.success, "{remaining:?}");
    let StatementResult::Query(query) = &remaining.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(query.row_count, 1);
    assert_eq!(query.rows[0]["id"], json!("doc-b"));
}

#[tokio::test]
async fn drop_text_collection_removes_table_and_registry() {
    let (_dir, path) = temp_db_path("drop_collection.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let dropped = exec
        .drop_text_collection(DropTextCollectionInput {
            collection: "docs".to_string(),
        })
        .await;

    assert!(dropped.success, "{dropped:?}");
    assert_eq!(dropped.data["collection"], json!("docs"));
    assert_eq!(dropped.data["existed"], json!(true));

    let registry = exec
        .execute("SELECT COUNT(*) AS count FROM __vector_collections;".to_string())
        .await;
    assert!(registry.success, "{registry:?}");
    let StatementResult::Query(registry_query) = &registry.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(registry_query.rows[0]["count"], json!(0));

    let table = exec.execute("SELECT id FROM vec_docs;".to_string()).await;
    assert!(!table.success, "{table:?}");

    let dropped_again = exec
        .drop_text_collection(DropTextCollectionInput {
            collection: "docs".to_string(),
        })
        .await;
    assert!(dropped_again.success, "{dropped_again:?}");
    assert_eq!(dropped_again.data["existed"], json!(false));
}

#[tokio::test]
async fn readonly_allows_search_and_rejects_vector_writes() {
    let (_dir, path) = temp_db_path("readonly_vectors.db");
    {
        let exec = executor(path.clone(), RunMode::Readwrite, 500, 100).await;
        let create = exec
            .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
                collection: "docs".to_string(),
                dimension: 2,
            })
            .await;
        assert!(create.success, "{create:?}");
        let upsert = exec
            .upsert_generated_texts(UpsertGeneratedTextsInput {
                collection: "docs".to_string(),
                items: vec![GeneratedTextItemInput {
                    id: "doc-a".to_string(),
                    vector: vec![1.0, 0.0],
                    text: "alpha".to_string(),
                    metadata: Some(json!({"tenant": "a"})),
                }],
            })
            .await;
        assert!(upsert.success, "{upsert:?}");
    }

    let readonly = executor(path, RunMode::Readonly, 500, 100).await;
    let search = readonly
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
        })
        .await;
    assert!(search.success, "{search:?}");
    assert_eq!(
        search.data["results"].as_array().unwrap()[0]["id"],
        json!("doc-a")
    );

    let create = readonly
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "other".to_string(),
            dimension: 2,
        })
        .await;
    assert!(!create.success, "{create:?}");
    assert!(vector_error_message(&create).contains("create_text_collection"));

    let upsert = readonly
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-b".to_string(),
                vector: vec![0.0, 1.0],
                text: "text".to_string(),
                metadata: None,
            }],
        })
        .await;
    assert!(!upsert.success, "{upsert:?}");
    assert!(vector_error_message(&upsert).contains("upsert_texts"));

    let delete = readonly
        .delete_texts(DeleteTextsInput {
            collection: "docs".to_string(),
            ids: vec!["doc-a".to_string()],
        })
        .await;
    assert!(!delete.success, "{delete:?}");
    assert!(vector_error_message(&delete).contains("delete_texts"));

    let drop = readonly
        .drop_text_collection(DropTextCollectionInput {
            collection: "docs".to_string(),
        })
        .await;
    assert!(!drop.success, "{drop:?}");
    assert!(vector_error_message(&drop).contains("drop_text_collection"));
}

#[tokio::test]
async fn readonly_search_missing_collection_returns_not_found() {
    let (_dir, path) = temp_db_path("readonly_missing_collection.db");
    {
        let exec = executor(path.clone(), RunMode::Readwrite, 500, 100).await;
        let response = exec.execute("SELECT 1;".to_string()).await;
        assert!(response.success, "{response:?}");
    }

    let readonly = executor(path, RunMode::Readonly, 500, 100).await;
    let search = readonly
        .search_generated_text(SearchGeneratedTextInput {
            collection: "docs".to_string(),
            vector: vec![1.0, 0.0],
            top_k: 1,
            filter: None,
        })
        .await;

    assert!(!search.success, "{search:?}");
    assert!(vector_error_message(&search).contains("collection not found"));
}

#[tokio::test]
async fn execute_sql_can_query_vector_collection_tables() {
    let (_dir, path) = temp_db_path("sql_compat.db");
    let exec = executor(path, RunMode::Readwrite, 500, 100).await;

    let create = exec
        .create_text_collection_with_dimension(CreateTextCollectionStorageInput {
            collection: "docs".to_string(),
            dimension: 2,
        })
        .await;
    assert!(create.success, "{create:?}");

    let upsert = exec
        .upsert_generated_texts(UpsertGeneratedTextsInput {
            collection: "docs".to_string(),
            items: vec![GeneratedTextItemInput {
                id: "doc-a".to_string(),
                vector: vec![1.0, 0.0],
                text: "alpha".to_string(),
                metadata: Some(json!({"tenant": "a"})),
            }],
        })
        .await;
    assert!(upsert.success, "{upsert:?}");

    let query = exec
        .execute(
            "SELECT c.name, c.table_name, v.id, v.text, v.metadata
             FROM __vector_collections c
             JOIN vec_docs v ON c.table_name = 'vec_docs'
             WHERE c.name = 'docs';"
                .to_string(),
        )
        .await;
    assert!(query.success, "{query:?}");
    let StatementResult::Query(result) = &query.results[0] else {
        panic!("expected query result");
    };
    assert_eq!(result.row_count, 1);
    assert_eq!(result.rows[0]["name"], json!("docs"));
    assert_eq!(result.rows[0]["table_name"], json!("vec_docs"));
    assert_eq!(result.rows[0]["id"], json!("doc-a"));
    assert_eq!(result.rows[0]["text"], json!("alpha"));
    assert_eq!(result.rows[0]["metadata"], json!(r#"{"tenant":"a"}"#));
}

fn vector_error_message(response: &sqlite_mcp_rs::vector::VectorToolResponse) -> &str {
    response.error.as_ref().unwrap().message.as_str()
}
