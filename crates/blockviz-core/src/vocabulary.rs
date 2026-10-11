//! What identifiers and libraries mean, shared by every language front end.
//!
//! Names are evidence, not proof: `head` is a projection in a transformer and an
//! HTTP verb in a client library. So name-based meanings apply only inside a
//! domain the code demonstrably works in (it imports a library of that domain),
//! and names are matched by whole words, read from the head noun backwards
//! (`attn_dropout` is a dropout, `c_attn` an attention projection).

/// Domains a module works in, from the libraries it (or a project module it
/// imports) uses.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Domains {
    /// Neural networks: architecture-figure colours apply.
    pub ml: bool,
    /// A database: `session.commit()` and `.objects.filter()` are queries.
    pub database: bool,
}

impl Domains {
    pub fn union(self, o: Domains) -> Domains {
        Domains { ml: self.ml || o.ml, database: self.database || o.database }
    }
}

/// Top-level library names (Python packages and npm packages).
const ML: &[&str] = &[
    "torch", "jax", "flax", "haiku", "equinox", "optax", "tensorflow", "keras", "mlx", "paddle", "mindspore", "tinygrad", "transformers", "diffusers", "timm", "lightning", "pytorch_lightning", "accelerate", "deepspeed", "triton", "einops", "xformers", "flash_attn", "torchvision", "torchaudio", "peft", "trl", "vllm", "megatron", "apex", "@tensorflow/tfjs", "@tensorflow/tfjs-node", "onnxruntime-web", "onnxruntime-node", "@xenova/transformers", "@huggingface/transformers",
    "candle", "candle_core", "candle_nn", "candle_transformers", "tch", "burn", "burn_tensor", "burn_core", "dfdx", "ort", "tract_onnx", "rust_bert", "mistralrs", "luminal",
];
const DATABASE: &[&str] = &[
    "sqlalchemy", "sqlmodel", "sqlite3", "psycopg", "psycopg2", "asyncpg", "aiosqlite", "aiomysql", "pymysql", "mysql", "MySQLdb", "pymongo", "motor", "mongoengine", "beanie", "odmantic", "peewee", "pony", "tortoise", "django", "databases", "duckdb", "redis", "aioredis", "cassandra", "neo4j", "edgedb", "prisma", "supabase", "sqlite_utils", "records", "dataset", "pg", "mysql2", "better-sqlite3", "mongoose", "mongodb", "@prisma/client", "typeorm", "sequelize", "knex", "drizzle-orm", "kysely", "ioredis", "@supabase/supabase-js",
    "sqlx", "diesel", "diesel_async", "sea_orm", "rusqlite", "tokio_rusqlite", "tokio_postgres", "postgres", "mysql_async", "deadpool_postgres", "deadpool_redis", "bb8", "r2d2", "sled", "rocksdb", "surrealdb", "scylla", "tiberius", "libsql", "fred", "rbatis", "lancedb", "qdrant_client",
];

/// The domains a library belongs to, by its top-level name.
pub fn library_domains(root: &str) -> Domains {
    Domains { ml: ML.contains(&root), database: DATABASE.contains(&root) }
}

/// Splits an identifier into lowercase words: `CausalSelfAttention` →
/// causal, self, attention; `c_attn` → c, attn; `BatchNorm2d` → batch, norm2d.
pub fn words(ident: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = ident.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        // A new word starts at an upper-case letter after a lower-case one, or
        // at the last capital of an acronym (`HTTPServer` → http, server).
        let boundary = c.is_uppercase()
            && !cur.is_empty()
            && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit() || chars.get(i + 1).is_some_and(|n| n.is_lowercase()) && chars[i - 1].is_uppercase());
        if boundary {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Architecture-figure category of a neural-network name: `LayerNorm` → norm,
/// `c_attn` → attention, `lm_head` → linear. Only meaningful in ML code.
pub fn ml_category(name: &str) -> Option<&'static str> {
    let ws = words(name);
    let whole: String = ws.concat();
    if let Some(c) = ml_word(&whole) {
        return Some(c);
    }
    // The head noun comes last: `attn_dropout` is a dropout.
    for i in (0..ws.len()).rev() {
        if i > 0 {
            if let Some(c) = ml_word(&format!("{}{}", ws[i - 1], ws[i])) {
                return Some(c);
            }
        }
        if let Some(c) = ml_word(&ws[i]) {
            return Some(c);
        }
    }
    None
}

fn ml_word(w: &str) -> Option<&'static str> {
    // `conv2d`, `fc1`, `norm2d`: the dimension or index does not change the kind.
    let w = w.trim_end_matches(|c: char| c.is_ascii_digit());
    let w = w.strip_suffix("1d").or_else(|| w.strip_suffix("2d")).or_else(|| w.strip_suffix("3d")).unwrap_or(w);
    let w = w.trim_end_matches(|c: char| c.is_ascii_digit());
    Some(match w {
        "attn" | "attention" | "selfattention" | "mha" | "sdpa" | "multiheadattention" => "attention",
        "norm" | "layernorm" | "rmsnorm" | "batchnorm" | "groupnorm" | "instancenorm" | "ln" | "rms" => "norm",
        "embed" | "embedding" | "embeddings" | "embedder" | "wte" | "wpe" => "embed",
        "softmax" | "gelu" | "relu" | "silu" | "selu" | "tanh" | "sigmoid" | "swish" | "mish" | "elu" | "glu" | "swiglu" | "geglu" | "act" | "activation" | "relu6" => "activation",
        "dropout" => "dropout",
        "mlp" | "ffn" | "ff" | "feedforward" => "mlp",
        "linear" | "proj" | "projection" | "dense" | "head" | "fc" | "conv" | "convolution" | "convtranspose" => "linear",
        "loss" | "crossentropy" | "nll" | "mse" | "bce" | "nllloss" => "loss",
        _ => return None,
    })
}

/// Functions and attributes that read a property of a value rather than its
/// content (its size, shape, type or existence), and clock readings. Their
/// results are metadata and their inputs are used only for that property, so
/// those edges are drawn lightly, with settings.
/// Attributes that are a property of a value rather than its content:
/// `x.shape`, `x.device`, `items.length`.
pub fn is_metadata_attr(name: &str) -> bool {
    matches!(name, "shape" | "ndim" | "dtype" | "device" | "is_cuda" | "size" | "length" | "byteLength" | "itemsize" | "nbytes")
}

pub fn is_metadata(name: &str) -> bool {
    matches!(
        name,
        "len" | "size" | "numel" | "dim" | "ndim" | "ndimension" | "shape" | "dtype" | "device" | "is_cuda" | "isinstance" | "issubclass" | "hasattr" | "callable" | "type" | "id" | "exists" | "is_file" | "is_dir" | "isfile" | "isdir" | "getsize" | "time" | "perf_counter" | "monotonic" | "time_ns" | "length" | "byteLength" | "isArray" | "typeof"
    )
}

/// Methods that query or change a database through a session, connection,
/// cursor or ORM manager.
const DB_VERBS: &[&str] = &[
    "execute", "executemany", "executescript", "commit", "rollback", "add", "add_all", "delete", "merge", "flush", "refresh", "query", "get", "exec", "scalar", "scalars", "fetchone", "fetchall", "fetchmany", "filter", "filter_by", "exclude", "all", "first", "create", "get_or_create", "update_or_create", "bulk_create", "bulk_update", "update", "insert_one", "insert_many", "find", "find_one", "update_one", "update_many", "delete_one", "delete_many", "aggregate", "count", "values", "values_list", "order_by", "select_related", "prefetch_related", "annotate", "findMany", "findUnique", "findFirst", "upsert", "insert", "select", "where",
    // Rust: sqlx, diesel, rusqlite, sea-orm.
    "fetch_one", "fetch_all", "fetch_optional", "fetch", "query_row", "query_map", "execute_batch", "get_result", "get_results", "load", "one",
];

/// Methods that query or change a database (or an ORM model).
pub fn is_database_verb(method: &str) -> bool {
    DB_VERBS.contains(&method) || matches!(method, "save" | "remove" | "deleteOne" | "deleteMany" | "updateOne" | "updateMany" | "findOne" | "findById" | "findByIdAndUpdate" | "findByIdAndDelete" | "findOneAndUpdate" | "findOneAndDelete" | "countDocuments" | "insertMany" | "paginate" | "populate" | "findUnique" | "findFirst" | "findMany" | "createMany" | "upsert" | "transaction" | "raw" | "aggregate" | "bulkWrite")
}

/// A database call by the shape of its receiver (`session.commit()`,
/// `Item.objects.filter()`, `conn.execute()`). Only meaningful in modules that
/// use a database library.
pub fn database_call(receiver: &str, method: &str) -> bool {
    if !DB_VERBS.contains(&method) {
        return false;
    }
    let r = receiver.to_ascii_lowercase();
    if r.ends_with(".objects") || r == "objects" || r.contains(".objects.") {
        return true;
    }
    let dbish = |n: &str| {
        let ws = words(n);
        let last = ws.last().map(String::as_str).unwrap_or("");
        matches!(n, "db" | "sess" | "cur" | "cnx" | "tx" | "conn") || matches!(last, "session" | "db" | "conn" | "connection" | "cursor" | "transaction" | "database" | "engine" | "prisma" | "pool")
    };
    let root = r.split('.').next().unwrap_or(&r);
    let last = r.rsplit('.').next().unwrap_or(&r);
    dbish(root) || dbish(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_identifiers() {
        assert_eq!(words("CausalSelfAttention"), ["causal", "self", "attention"]);
        assert_eq!(words("c_attn"), ["c", "attn"]);
        assert_eq!(words("HTTPServer"), ["http", "server"]);
        assert_eq!(words("BatchNorm2d"), ["batch", "norm2d"]);
        assert_eq!(words("lm_head"), ["lm", "head"]);
    }

    #[test]
    fn categories_by_head_noun() {
        assert_eq!(ml_category("attn_dropout"), Some("dropout"));
        assert_eq!(ml_category("c_attn"), Some("attention"));
        assert_eq!(ml_category("ln_f"), Some("norm"));
        assert_eq!(ml_category("LayerNorm"), Some("norm"));
        assert_eq!(ml_category("BatchNorm2d"), Some("norm"));
        assert_eq!(ml_category("FeedForward"), Some("mlp"));
        assert_eq!(ml_category("cross_entropy"), Some("loss"));
        assert_eq!(ml_category("SiLU"), Some("activation"));
        assert_eq!(ml_category("fc1"), Some("linear"));
        // Substrings are not words.
        assert_eq!(ml_category("headers"), None);
        assert_eq!(ml_category("normalize_url"), None);
        assert_eq!(ml_category("conversation"), None);
        assert_eq!(ml_category("lossless"), None);
        assert_eq!(ml_category("embedded"), None);
    }

    #[test]
    fn database_receivers() {
        assert!(database_call("session", "commit"));
        assert!(database_call("db_session", "add"));
        assert!(database_call("Item.objects", "filter"));
        assert!(database_call("self.conn", "execute"));
        assert!(!database_call("sessions", "get"));
        assert!(!database_call("headers", "get"));
    }
}
