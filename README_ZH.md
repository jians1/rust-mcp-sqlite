# sqlite-mcp-rs

[English](README.md) | 中文

基于 Streamable HTTP 的 Rust SQLite MCP 服务器。

当 MCP 客户端需要通过 SQL 和可选的向量集合工具来检查或修改单个 SQLite 数据库文件时使用。

## 快速开始

从源码构建：

```bash
cargo build --release
```

启动本地读写服务器：

```bash
./target/release/sqlite-mcp-rs \
  --db ./app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite
```

MCP 端点为：

```text
http://127.0.0.1:3000/mcp
```

然后配置你的 MCP 客户端使用 Streamable HTTP 和该 URL。服务器暴露以下工具：

```text
execute_sql
create_vector_collection
upsert_vectors
search_vectors
delete_vectors
drop_vector_collection
```

工具概述：

| 工具 | 用途 | 模式 |
| --- | --- | --- |
| `execute_sql` | 对配置的数据库运行 SQLite SQL。 | 在 `readonly` 模式下读取；在 `readwrite` 模式下读写。 |
| `create_vector_collection` | 创建命名的 sqlite-vec 集合。 | 仅 `readwrite`。 |
| `upsert_vectors` | 插入或替换客户端提供的向量。 | 仅 `readwrite`。 |
| `search_vectors` | 按余弦距离搜索集合。 | `readonly` 和 `readwrite`。 |
| `delete_vectors` | 按 id 删除向量记录。 | 仅 `readwrite`。 |
| `drop_vector_collection` | 删除向量集合及注册表元数据。 | 仅 `readwrite`。 |

## 安装

### 从 GitHub Releases 安装

从以下地址下载匹配的 Linux 资源：

```text
https://github.com/jians1/rust-mcp-sqlite/releases
```

发布资源打包为 tarball，例如：

```text
sqlite-mcp-rs-v0.1-linux-amd64.tar.gz
sqlite-mcp-rs-v0.1-linux-arm64.tar.gz
sqlite-mcp-rs-v0.1-linux-amd64-musl.tar.gz
sqlite-mcp-rs-v0.1-linux-arm64-musl.tar.gz
```

解压归档并运行其中的 `sqlite-mcp-rs` 二进制文件。

### 从源码安装

要求：

- Rust stable
- Cargo

构建：

```bash
cargo build --release
```

运行：

```bash
./target/release/sqlite-mcp-rs --db ./app.db
```

或安装到 Cargo 的 bin 目录：

```bash
cargo install --path .
sqlite-mcp-rs --db ./app.db
```

SQLite 通过 `rusqlite` 捆绑，因此正常构建不需要系统 SQLite 安装。

## 运行

最小本地服务器：

```bash
sqlite-mcp-rs --db ./app.db
```

带鉴权的生产风格本地后端：

```bash
export MCP_AUTH_TOKEN='change-me'

sqlite-mcp-rs \
  --db /data/app.db \
  --host 127.0.0.1 \
  --port 3000 \
  --mode readwrite \
  --auth-token "$MCP_AUTH_TOKEN" \
  --max-rows 500 \
  --max-top-k 100 \
  --timeout-ms 10000
```

只读服务器：

```bash
sqlite-mcp-rs \
  --db /data/app.db \
  --mode readonly \
  --auth-token "$MCP_AUTH_TOKEN"
```

命令行选项：

| 选项 | 默认值 | 描述 |
| --- | --- | --- |
| `--db <path>` | 必需 | SQLite 数据库文件。`readwrite` 模式可能创建文件；`readonly` 模式要求现有可读文件。 |
| `--host <ip>` | `127.0.0.1` | 监听地址。在反向代理后时保持 localhost。 |
| `--port <port>` | `3000` | 监听端口。 |
| `--mode <mode>` | `readwrite` | `readonly` 或 `readwrite`。 |
| `--auth-token <token>` | 无 | 为每个 HTTP 请求启用 Bearer token 鉴权。 |
| `--max-rows <n>` | `500` | 每条产生行的 SQL 语句最多返回的行数。 |
| `--max-top-k <n>` | `100` | `search_vectors` 接受的最大 `top_k`。 |
| `--timeout-ms <n>` | `10000` | 整个 `execute_sql` 调用的超时时间。 |

## MCP 客户端配置

使用 Streamable HTTP 传输并将客户端指向 `/mcp`：

```json
{
  "mcpServers": {
    "sqlite": {
      "type": "http",
      "url": "http://127.0.0.1:3000/mcp"
    }
  }
}
```

如果启用了 `--auth-token`，发送：

```http
Authorization: Bearer change-me
```

一些 MCP 客户端支持在配置中设置 headers：

```json
{
  "mcpServers": {
    "sqlite": {
      "type": "http",
      "url": "http://127.0.0.1:3000/mcp",
      "headers": {
        "Authorization": "Bearer change-me"
      }
    }
  }
}
```

具体配置键因 MCP 客户端而异。重要的部分是：

- 传输：Streamable HTTP
- URL：`http://<host>:<port>/mcp`
- 可选 header：`Authorization: Bearer <token>`

## 使用 curl 进行冒烟测试

首先启动服务器：

```bash
sqlite-mcp-rs --db /tmp/sqlite-mcp-smoke.db --port 3000 --mode readwrite
```

初始化 MCP 会话：

```bash
curl -sS \
  -H 'accept: application/json, text/event-stream' \
  -H 'content-type: application/json' \
  --data-binary @- \
  http://127.0.0.1:3000/mcp <<'JSON'
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2025-06-18",
    "capabilities": {},
    "clientInfo": {"name": "curl", "version": "0.1.0"}
  }
}
JSON
```

调用 `execute_sql`：

```bash
curl -sS \
  -H 'accept: application/json, text/event-stream' \
  -H 'content-type: application/json' \
  --data-binary @- \
  http://127.0.0.1:3000/mcp <<'JSON'
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "execute_sql",
    "arguments": {
      "sql": "CREATE TABLE IF NOT EXISTS smoke(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO smoke(name) VALUES ('alpha'), ('beta'); SELECT id, name FROM smoke ORDER BY id;"
    }
  }
}
JSON
```

响应是一个 MCP `content` 项，其 `text` 字段是一个 JSON 字符串。解析该文本以读取工具结果。

启用鉴权时，添加：

```bash
-H "Authorization: Bearer $MCP_AUTH_TOKEN"
```

## 工具：execute_sql

输入模式：

```json
{
  "sql": "字符串，必需"
}
```

单条语句：

```json
{"sql": "SELECT 1 AS value"}
```

多条语句放在同一个 `sql` 字符串中：

```json
{
  "sql": "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO users(name) VALUES ('alice'); SELECT id, name FROM users;"
}
```

支持的 SQL 包括：

- `SELECT`、`EXPLAIN` 和读取式 `PRAGMA`
- 在 `readwrite` 模式下的 `INSERT`、`UPDATE`、`DELETE` 和 `REPLACE`
- 在 `readwrite` 模式下的 `CREATE`、`DROP` 和 `ALTER`
- 使用 `WITH` 的公共表表达式
- 带 `RETURNING` 的语句
- FTS5
- sqlite-vec 函数和 `vec0` 虚拟表

不要包含显式事务控制语句：

- `BEGIN`
- `COMMIT`
- `ROLLBACK`
- `SAVEPOINT`
- `RELEASE`

每次 `execute_sql` 调用都由服务器包装在一个事务中。如果调用中的任何语句失败，整个调用会回滚，`results` 为空。

## execute_sql 响应格式

MCP 工具返回一个文本内容项。文本是一个 JSON 封装。

成功响应：

```json
{
  "success": true,
  "results": [
    {
      "statement_index": 0,
      "statement_type": "SELECT",
      "columns": ["value"],
      "rows": [{"value": 1}],
      "row_count": 1,
      "truncated": false
    }
  ],
  "elapsed_ms": 0
}
```

失败响应：

```json
{
  "success": false,
  "error": {
    "message": "no such table: users",
    "statement_index": 0
  },
  "results": [],
  "elapsed_ms": 3
}
```

结果形状：

- 查询返回 `columns`、`rows`、`row_count` 和 `truncated`
- `INSERT` 返回 `affected_rows` 和 `last_insert_rowid`
- `UPDATE` 和 `DELETE` 返回 `affected_rows`
- 模式更改返回 `success` 和 `schema_changed`
- 其他合法的非查询语句返回通用成功结果

SQLite 值映射：

- `NULL` -> `null`
- `INTEGER` 和 `REAL` -> JSON 数字
- `TEXT` -> JSON 字符串
- `BLOB` -> `{"type":"blob","encoding":"base64","data":"..."}`

## 向量集合

向量支持使用带有 `sqlite-vec` 的 SQLite。向量由客户端以 JSON 数字数组形式提供；本服务器不生成向量或调用模型 API。集合使用余弦距离，存储为名为 `vec_<collection>` 的 `vec0` 虚拟表。

集合名称必须仅包含 ASCII 字母、数字和下划线，且不能以 `__` 开头。每条记录具有：

- `id`：非空字符串
- `vector`：匹配集合维度的 JSON 数字数组
- `text`：可选字符串
- `metadata`：可选 JSON 对象，省略时存储为 `{}`

向量工具返回与 `execute_sql` 相同的 MCP 形状：包含 JSON 封装的文本内容项。成功的向量封装包括：

- `success`：`true`
- `collection`：集合名称（如果相关）
- 操作特定字段，如 `created`、`upserted_count`、`results`、`requested_count`、`deleted_count` 或 `existed`
- `elapsed_ms`

失败的向量封装包括：

```json
{
  "success": false,
  "error": {
    "message": "vector dimension mismatch: expected 1536, got 768"
  },
  "elapsed_ms": 0
}
```

### create_vector_collection

```json
{
  "collection": "docs",
  "dimension": 1536
}
```

创建 `vec_docs` 并在 `__vector_collections` 中记录元数据。使用相同维度再次调用会成功并返回 `"created": false`；不同维度会返回错误。

示例成功响应：

```json
{
  "success": true,
  "collection": "docs",
  "table_name": "vec_docs",
  "dimension": 1536,
  "distance_metric": "cosine",
  "created": true,
  "elapsed_ms": 3
}
```

### upsert_vectors

```json
{
  "collection": "docs",
  "items": [
    {
      "id": "doc-1",
      "vector": [0.12, -0.03, 0.88],
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ]
}
```

Upsert 会替换相同 `id` 的整条记录：向量、文本和元数据。批次是原子性的。

验证规则：

- `items` 可以包含一条或多条记录。
- `id` 必须非空。
- `vector` 长度必须匹配集合维度。
- 向量值必须是有限的 JSON 数字。
- `metadata` 存在时必须是 JSON 对象。

### search_vectors

```json
{
  "collection": "docs",
  "vector": [0.12, -0.03, 0.88],
  "top_k": 5,
  "filter": {"tenant": "a", "source": "manual"}
}
```

结果包括 `id`、`distance`、`text` 和 `metadata`。默认不返回存储的向量。`top_k` 必须是正数且不大于 `--max-top-k`。

过滤器是可选的顶层元数据相等性检查。过滤器键必须是简单标识符，值必须是标量 JSON 值：字符串、数字、布尔值或 null。不支持嵌套路径、数组、对象、范围和包含查询。

无过滤的搜索使用 sqlite-vec KNN。有过滤的搜索首先应用精确的 JSON 元数据过滤，然后按余弦距离对过滤后的行进行排序；有过滤的搜索是正确的，但在此版本中未针对 KNN 优化。

示例成功响应：

```json
{
  "success": true,
  "collection": "docs",
  "results": [
    {
      "id": "doc-1",
      "distance": 0.0,
      "text": "chunk text",
      "metadata": {"source": "manual", "tenant": "a"}
    }
  ],
  "elapsed_ms": 2
}
```

### delete_vectors

```json
{
  "collection": "docs",
  "ids": ["doc-1", "doc-2"]
}
```

删除匹配的 id 并返回 `requested_count` 和 `deleted_count`。缺失的 id 不是错误。

### drop_vector_collection

```json
{
  "collection": "docs"
}
```

删除集合表并移除其注册表行。删除缺失的集合会成功并返回 `"existed": false`。

### SQL 检查

向量工具是 SQLite 状态的便利包装。高级用户可以通过 `execute_sql` 检查注册表和集合表：

```json
{
  "sql": "SELECT name, table_name, dimension, distance_metric, created_at FROM __vector_collections; SELECT id, text, metadata FROM vec_docs LIMIT 10;"
}
```

也可以直接使用 sqlite-vec 函数查询向量表。这适用于由向量工具创建的集合：

```json
{
  "sql": "SELECT id, distance FROM vec_docs WHERE embedding MATCH vec_f32('[0.12,-0.03,0.88]') ORDER BY distance LIMIT 5;"
}
```

高级用户还可以通过 `execute_sql` 直接创建 sqlite-vec 表：

```json
{
  "sql": "CREATE VIRTUAL TABLE vec_direct USING vec0(id TEXT PRIMARY KEY, embedding float[2] distance_metric=cosine, +text TEXT, +metadata TEXT); INSERT INTO vec_direct(id, embedding, text, metadata) VALUES ('doc-a', vec_f32('[1.0,0.0]'), 'alpha', '{\"tenant\":\"a\"}'); SELECT id, distance FROM vec_direct WHERE embedding MATCH vec_f32('[1.0,0.0]') ORDER BY distance LIMIT 1;"
}
```

以这种方式直接创建的表不会注册到 `__vector_collections`，因此向量便利工具不会管理它们，除非你也维护兼容的注册表元数据。

### 最小 MCP 工作流

对于原始 JSON-RPC 客户端，每个向量操作都使用 `tools/call` 调用。`arguments` 对象是上面显示的工具输入。

创建集合：

```json
{
  "jsonrpc": "2.0",
  "id": 10,
  "method": "tools/call",
  "params": {
    "name": "create_vector_collection",
    "arguments": {"collection": "docs", "dimension": 2}
  }
}
```

插入和搜索：

```json
{
  "jsonrpc": "2.0",
  "id": 11,
  "method": "tools/call",
  "params": {
    "name": "upsert_vectors",
    "arguments": {
      "collection": "docs",
      "items": [
        {
          "id": "doc-a",
          "vector": [1.0, 0.0],
          "text": "alpha",
          "metadata": {"tenant": "a"}
        }
      ]
    }
  }
}
```

```json
{
  "jsonrpc": "2.0",
  "id": 12,
  "method": "tools/call",
  "params": {
    "name": "search_vectors",
    "arguments": {
      "collection": "docs",
      "vector": [1.0, 0.0],
      "top_k": 1,
      "filter": {"tenant": "a"}
    }
  }
}
```

## 模式与安全

### readonly

`readonly` 以只读方式打开 SQLite 数据库并拒绝修改语句。当 MCP 客户端应该检查数据但不更改数据时使用。

允许的示例：

```sql
SELECT * FROM users LIMIT 10;
PRAGMA table_info(users);
```

`search_vectors` 在 readonly 模式下也是允许的。

拒绝的示例：

```sql
INSERT INTO users(name) VALUES ('alice');
CREATE TABLE t(id INTEGER);
PRAGMA user_version = 1;
```

`create_vector_collection`、`upsert_vectors`、`delete_vectors` 和 `drop_vector_collection` 在 readonly 模式下被拒绝。

### readwrite

`readwrite` 允许合法的 SQLite SQL，但显式事务控制语句除外。仅对受信任的客户端使用。

服务器通过一个 SQLite 连接串行化 SQL 执行。并发的 HTTP 请求会被接受，但数据库工作按顺序执行。

## 部署说明

推荐的部署形式：

```text
MCP 客户端 -> HTTPS 反向代理 -> sqlite-mcp-rs 在 127.0.0.1:<port> -> SQLite 文件
```

使用 Nginx 或 Caddy 实现：

- HTTPS
- 域名路由
- IP 白名单
- 额外的身份验证或访问策略

即使存在反向代理，也使用 `--auth-token` 进行后端 Bearer 鉴权。

保持数据库文件及其目录权限仅限于服务用户。

## 故障排除

`401 Unauthorized`

- `--auth-token` 已启用。
- 向 MCP 客户端或 curl 请求添加 `Authorization: Bearer <token>`。

`readonly mode forbids ... statements`

- 服务器使用 `--mode readonly` 运行。
- 仅在打算写入时使用 `--mode readwrite` 重启。

`transaction control statements are not allowed`

- 移除 `BEGIN`、`COMMIT`、`ROLLBACK`、`SAVEPOINT` 或 `RELEASE`。
- 将 SQL 语句一起发送；服务器处理事务。

`query timed out after ... ms`

- 增加 `--timeout-ms`，减少查询成本，或添加索引。

查询结果中缺少太多行

- 增加 `--max-rows`。
- 检查结果是否有 `"truncated": true`。

## 开发

运行测试：

```bash
cargo test
```

带日志运行：

```bash
RUST_LOG=info cargo run -- --db ./app.db --mode readwrite
```
