//! SQLite ledger backend (feature `ledger-sqlite`).

use std::str::FromStr;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::ledger::{LedgerError, LedgerStore, UsageEntry};

pub struct SqliteLedger {
    pool: SqlitePool,
}

impl SqliteLedger {
    /// Connect (DSN like `sqlite://synapse.db?mode=rwc` or `sqlite::memory:`)
    /// and create the table if absent.
    ///
    /// Uses `max_connections(1)` so that both file-backed and in-memory databases
    /// work correctly: with `sqlite::memory:` every connection gets its own
    /// isolated database, so a single connection ensures the migration and all
    /// subsequent writes share the same in-memory DB.
    pub async fn connect(dsn: &str) -> Result<Self, LedgerError> {
        let opts = SqliteConnectOptions::from_str(dsn)
            .map_err(|e| LedgerError::Backend(e.to_string()))?
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| LedgerError::Backend(e.to_string()))?;

        // Run the multi-statement migration via raw_sql which supports
        // multiple `;`-separated statements in a single call.
        sqlx::raw_sql(include_str!("../../migrations/0001_usage_events.sql"))
            .execute(&pool)
            .await
            .map_err(|e| LedgerError::Backend(e.to_string()))?;

        // Best-effort for databases created before the user_id column existed;
        // SQLite has no ADD COLUMN IF NOT EXISTS, so ignore "duplicate column".
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN user_id TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN thread_id TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN message_id TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN user_task_type TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN ai_task_type TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN score REAL")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN confidence REAL")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN tier TEXT")
            .execute(&pool)
            .await;
        let _ = sqlx::query("ALTER TABLE usage_events ADD COLUMN outcome TEXT")
            .execute(&pool)
            .await;

        Ok(Self { pool })
    }
}

#[async_trait]
impl LedgerStore for SqliteLedger {
    async fn record(&self, e: &UsageEntry) -> Result<(), LedgerError> {
        sqlx::query(
            "INSERT INTO usage_events \
             (ts, tenant, workspace, user_id, thread_id, message_id, route, provider, model, lane, \
              input_tokens, output_tokens, cost_usd, request_id, status, user_task_type, ai_task_type, \
              score, confidence, tier, outcome) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(e.ts.to_rfc3339())
        .bind(&e.tenant)
        .bind(&e.workspace)
        .bind(&e.user)
        .bind(&e.thread)
        .bind(&e.message)
        .bind(&e.route)
        .bind(&e.provider)
        .bind(&e.model)
        .bind(&e.lane)
        .bind(e.input_tokens as i64)
        .bind(e.output_tokens as i64)
        .bind(e.cost_usd)
        .bind(&e.request_id)
        .bind(&e.status)
        .bind(&e.user_task_type)
        .bind(&e.ai_task_type)
        .bind(e.score)
        .bind(e.confidence)
        .bind(&e.tier)
        .bind(&e.outcome)
        .execute(&self.pool)
        .await
        .map_err(|e| LedgerError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sqlx::Row;

    fn entry() -> UsageEntry {
        UsageEntry {
            ts: Utc::now(),
            tenant: "acme".into(),
            workspace: None,
            user: None,
            thread: None,
            message: None,
            route: "fast".into(),
            provider: "vertex".into(),
            model: "gemini-3-flash".into(),
            lane: "standard".into(),
            input_tokens: 3,
            output_tokens: 5,
            cost_usd: 0.001,
            request_id: "req-1".into(),
            status: "ok".into(),
            op: "chat".into(),
            user_task_type: None,
            ai_task_type: "simple".into(),
            score: None,
            confidence: None,
            tier: None,
            outcome: None,
        }
    }

    async fn stored_user_task_type(e: &UsageEntry) -> Option<String> {
        stored_column(e, "user_task_type").await
    }

    async fn stored_column(e: &UsageEntry, column: &str) -> Option<String> {
        let store = SqliteLedger::connect("sqlite::memory:").await.unwrap();
        store.record(e).await.unwrap();
        sqlx::query(&format!("SELECT {column} FROM usage_events"))
            .fetch_one(&store.pool)
            .await
            .unwrap()
            .get(column)
    }

    #[tokio::test]
    async fn persists_ai_task_type_column() {
        let e = UsageEntry {
            ai_task_type: "conversation".into(),
            ..entry()
        };
        assert_eq!(
            stored_column(&e, "ai_task_type").await,
            Some("conversation".to_string())
        );
    }

    #[tokio::test]
    async fn persists_user_task_type_column() {
        let e = UsageEntry {
            user_task_type: Some("summarisation".into()),
            ..entry()
        };
        assert_eq!(
            stored_user_task_type(&e).await,
            Some("summarisation".to_string())
        );
    }

    #[tokio::test]
    async fn stores_null_user_task_type_when_absent() {
        assert_eq!(stored_user_task_type(&entry()).await, None);
    }

    #[tokio::test]
    async fn persists_route_decision_columns() {
        let store = SqliteLedger::connect("sqlite::memory:").await.unwrap();
        let e = UsageEntry {
            op: "route_decision".into(),
            score: Some(0.84),
            confidence: Some(0.93),
            tier: Some("hard".into()),
            outcome: Some("decided".into()),
            ..entry()
        };
        store.record(&e).await.unwrap();
        let row = sqlx::query("SELECT score, confidence, tier, outcome FROM usage_events")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(row.get::<Option<f64>, _>("score"), Some(0.84));
        assert_eq!(row.get::<Option<f64>, _>("confidence"), Some(0.93));
        assert_eq!(row.get::<Option<String>, _>("tier"), Some("hard".into()));
        assert_eq!(
            row.get::<Option<String>, _>("outcome"),
            Some("decided".into())
        );
    }

    /// A database created before `user_task_type` existed is upgraded in place by
    /// `connect`, so writes from a new binary against an old file still land.
    #[tokio::test]
    async fn backfills_column_on_a_pre_existing_database() {
        let path = std::env::temp_dir().join(format!("synapse-{}.db", uuid::Uuid::new_v4()));
        let dsn = format!("sqlite://{}?mode=rwc", path.display());

        let legacy = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&dsn)
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE usage_events (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, tenant TEXT NOT NULL, \
             workspace TEXT, route TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, \
             lane TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, \
             cost_usd REAL NOT NULL, request_id TEXT NOT NULL, status TEXT NOT NULL)",
        )
        .execute(&legacy)
        .await
        .unwrap();
        legacy.close().await;

        let store = SqliteLedger::connect(&dsn).await.unwrap();
        let e = UsageEntry {
            user_task_type: Some("summarisation".into()),
            ai_task_type: "conversation".into(),
            ..entry()
        };
        store.record(&e).await.unwrap();
        let row = sqlx::query("SELECT user_task_type, ai_task_type FROM usage_events")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("user_task_type"),
            Some("summarisation".to_string())
        );
        assert_eq!(
            row.get::<Option<String>, _>("ai_task_type"),
            Some("conversation".to_string())
        );

        let _ = std::fs::remove_file(&path);
    }
}
