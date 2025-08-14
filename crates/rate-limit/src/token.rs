//! Token-based rate limiting for LLM requests.

use std::sync::Arc;
use std::time::Duration;

use config::{PerUserRateLimits, StorageConfig, TokenRateLimit, TokenRateLimitsConfig};

use crate::error::RateLimitError;
use crate::storage::{InMemoryStorage, RateLimitStorage, StorageError};

/// Request information for token-based rate limiting.
#[derive(Debug, Clone)]
pub struct TokenRateLimitRequest {
    /// Client identifier for rate limiting.
    pub client_id: String,
    /// Optional group identifier for hierarchical rate limiting.
    pub group: Option<String>,
    /// Provider name (e.g., "openai", "anthropic").
    pub provider: String,
    /// Model name (e.g., "gpt-4", "claude-3").
    pub model: Option<String>,
    /// Number of tokens to consume.
    pub tokens: usize,
}

/// Storage backend for token rate limiting.
enum Storage {
    Memory(InMemoryStorage),
    Redis(crate::storage::redis::RedisStorage),
}

impl Storage {
    async fn check_and_consume_tokens(
        &self,
        key: &str,
        tokens: u32,
        limit: u32,
        interval: Duration,
    ) -> Result<crate::storage::RateLimitResult, StorageError> {
        match self {
            Storage::Memory(storage) => storage.check_and_consume_tokens(key, tokens, limit, interval).await,
            Storage::Redis(storage) => storage.check_and_consume_tokens(key, tokens, limit, interval).await,
        }
    }
}

/// Manager for token-based rate limiting.
#[derive(Clone)]
pub struct TokenRateLimitManager {
    storage: Arc<Storage>,
}

impl TokenRateLimitManager {
    /// Create a new token rate limit manager with configured storage backend.
    pub async fn new(storage_config: &StorageConfig) -> Result<Self, RateLimitError> {
        let storage = match storage_config {
            StorageConfig::Memory => Storage::Memory(InMemoryStorage::new()),
            StorageConfig::Redis(redis_config) => {
                use crate::storage::redis::RedisStorage;
                let redis_storage = RedisStorage::new(redis_config).await.map_err(RateLimitError::Storage)?;
                Storage::Redis(redis_storage)
            }
        };

        Ok(Self {
            storage: Arc::new(storage),
        })
    }

    /// Check if a token request is allowed based on rate limits.
    ///
    /// Returns the duration to wait if rate limited, or None if allowed.
    pub async fn check_request(
        &self,
        request: &TokenRateLimitRequest,
        provider_limits: Option<&TokenRateLimitsConfig>,
        model_limits: Option<&TokenRateLimitsConfig>,
    ) -> Result<Option<Duration>, RateLimitError> {
        // Resolve the appropriate rate limit based on hierarchy
        let rate_limit = match resolve_token_rate_limit(request.group.as_deref(), provider_limits, model_limits) {
            Some(limit) => limit,
            None => {
                // No rate limit configured
                log::debug!("No token rate limit configured for client {}", request.client_id);
                return Ok(None);
            }
        };

        // Create storage key based on client, group, provider, and model
        let key = format!(
            "token:{}:{}:{}:{}",
            request.client_id,
            request.group.as_deref().unwrap_or("default"),
            request.provider,
            request.model.as_deref().unwrap_or("default")
        );

        log::debug!(
            "Checking token rate limit for key '{}': {} tokens against limit of {} per {:?}",
            key,
            request.tokens,
            rate_limit.limit,
            rate_limit.interval
        );

        // Check rate limit using the storage backend with token consumption
        let tokens_to_consume = request.tokens as u32;
        let token_limit = rate_limit.limit as u32;

        if tokens_to_consume == 0 || token_limit == 0 {
            // Edge case: no tokens requested or no limit set
            return Ok(None);
        }

        let result = self
            .storage
            .check_and_consume_tokens(&key, tokens_to_consume, token_limit, rate_limit.interval)
            .await
            .map_err(RateLimitError::Storage)?;

        if !result.allowed {
            // If retry_after is None, it means the request can never succeed (insufficient capacity)
            // Use Duration::MAX as a sentinel value to indicate the request should be rejected permanently
            let retry_after = result.retry_after.unwrap_or(std::time::Duration::MAX);

            log::debug!(
                "Token rate limit exceeded for client '{}': retry after {:?}",
                request.client_id,
                if retry_after == std::time::Duration::MAX {
                    "never (exceeds capacity)".to_string()
                } else {
                    format!("{:?}", retry_after)
                }
            );

            Ok(Some(retry_after))
        } else {
            log::debug!("Token request allowed for client '{}'", request.client_id);
            Ok(None)
        }
    }
}

/// Helper to convert PerUserRateLimits to TokenRateLimit for the default case.
fn per_user_to_token_limit(per_user: &PerUserRateLimits) -> TokenRateLimit {
    TokenRateLimit {
        limit: per_user.limit,
        interval: per_user.interval,
        output_buffer: per_user.output_buffer,
    }
}

/// Resolve the appropriate rate limit based on the 4-level hierarchy.
///
/// 1. Model-specific + Group-specific
/// 2. Model-specific default
/// 3. Provider-specific + Group-specific
/// 4. Provider-specific default
pub fn resolve_token_rate_limit(
    group: Option<&str>,
    provider_limits: Option<&TokenRateLimitsConfig>,
    model_limits: Option<&TokenRateLimitsConfig>,
) -> Option<TokenRateLimit> {
    // Level 1: Model-specific + Group-specific
    if let Some(model_limits) = model_limits
        && let Some(per_user) = &model_limits.per_user
    {
        if let Some(group) = group
            && let Some(limit) = per_user.groups.get(group)
        {
            log::debug!("Using model-specific group limit for group '{}'", group);
            return Some(limit.clone());
        }

        // Level 2: Model-specific default
        log::debug!("Using model-specific per-user default limit");
        return Some(per_user_to_token_limit(per_user));
    }

    // Level 3: Provider-specific + Group-specific
    if let Some(provider_limits) = provider_limits
        && let Some(per_user) = &provider_limits.per_user
    {
        if let Some(group) = group
            && let Some(limit) = per_user.groups.get(group)
        {
            log::debug!("Using provider-specific group limit for group '{}'", group);
            return Some(limit.clone());
        }

        // Level 4: Provider-specific default
        log::debug!("Using provider-specific per-user default limit");
        return Some(per_user_to_token_limit(per_user));
    }

    log::debug!("No token rate limit found in hierarchy");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_limits(default: Option<u64>, groups: Vec<(&str, u64)>) -> TokenRateLimitsConfig {
        let default_limit = default.unwrap_or(1000);
        TokenRateLimitsConfig {
            per_user: Some(PerUserRateLimits {
                limit: default_limit,
                interval: Duration::from_secs(60),
                output_buffer: Some(500),
                groups: groups
                    .into_iter()
                    .map(|(name, limit)| {
                        (
                            name.to_string(),
                            TokenRateLimit {
                                limit,
                                interval: Duration::from_secs(60),
                                output_buffer: Some(500),
                            },
                        )
                    })
                    .collect(),
            }),
        }
    }

    #[test]
    fn test_hierarchy_level_1_model_group() {
        let provider_limits = create_limits(Some(1000), vec![("pro", 2000)]);
        let model_limits = create_limits(Some(3000), vec![("pro", 4000)]);

        let limit = resolve_token_rate_limit(Some("pro"), Some(&provider_limits), Some(&model_limits));
        assert_eq!(limit.unwrap().limit, 4000); // Model + Group
    }

    #[test]
    fn test_hierarchy_level_2_model_default() {
        let provider_limits = create_limits(Some(1000), vec![("pro", 2000)]);
        let model_limits = create_limits(Some(3000), vec![("enterprise", 4000)]);

        let limit = resolve_token_rate_limit(Some("pro"), Some(&provider_limits), Some(&model_limits));
        assert_eq!(limit.unwrap().limit, 3000); // Model default
    }

    #[test]
    fn test_hierarchy_level_3_provider_group() {
        let provider_limits = create_limits(Some(1000), vec![("pro", 2000)]);

        let limit = resolve_token_rate_limit(Some("pro"), Some(&provider_limits), None);
        assert_eq!(limit.unwrap().limit, 2000); // Provider + Group
    }

    #[test]
    fn test_hierarchy_level_4_provider_default() {
        let provider_limits = create_limits(Some(1000), vec![("enterprise", 2000)]);

        let limit = resolve_token_rate_limit(Some("pro"), Some(&provider_limits), None);
        assert_eq!(limit.unwrap().limit, 1000); // Provider default
    }

    #[test]
    fn test_no_limits_configured() {
        let limit = resolve_token_rate_limit(Some("pro"), None, None);
        assert!(limit.is_none());
    }
}
