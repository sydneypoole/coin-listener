use coin_listener_core::{AppError, AppResult, PostgresConfig};
use sqlx::{postgres::PgPoolOptions, PgPool};

pub async fn connect_postgres(config: &PostgresConfig) -> AppResult<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}

pub async fn run_migrations(pool: &PgPool) -> AppResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
}
