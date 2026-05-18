use coin_listener_core::{AppError, AppResult, RedisConfig};
use redis::Client;

pub fn connect_redis(config: &RedisConfig) -> AppResult<Client> {
    Client::open(config.redis_url.as_str()).map_err(|error| AppError::Redis(error.to_string()))
}
