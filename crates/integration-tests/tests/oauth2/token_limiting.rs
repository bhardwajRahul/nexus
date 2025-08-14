//! Tests to create Hydra clients with custom metadata and test rate limiting.

use super::{HydraClient, RequestBuilderExt};
use indoc::indoc;
use integration_tests::{TestServer, llms::OpenAIMock};
use serde_json::json;

/// Helper to create a Hydra client with custom metadata.
async fn create_hydra_client_with_metadata(
    hydra: &HydraClient,
    client_id: &str,
    metadata: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let admin_url = hydra.public_url.replace("4444", "4445"); // Convert to admin URL

    let request_body = json!({
        "client_id": client_id,
        "client_secret": format!("{}-secret", client_id),
        "grant_types": ["client_credentials"],
        "token_endpoint_auth_method": "client_secret_basic",
        "access_token_strategy": "jwt",
        "skip_consent": true,
        "skip_logout_consent": true,
        "audience": ["http://127.0.0.1:8080"],
        "metadata": metadata
    });

    let response = hydra
        .client
        .post(format!("{}/admin/clients", admin_url))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        if !error_text.contains("already exists") && !error_text.contains("resource with that value exists already") {
            return Err(format!("Failed to create client: {}", error_text).into());
        }
        // Client already exists, which is fine for our tests
    }

    Ok(())
}

/// Test creating different clients and using their client_id as the identifier.
#[tokio::test]
async fn test_rate_limiting_with_different_client_ids() {
    let hydra = HydraClient::new(4444, 4445);
    hydra.wait_for_hydra().await.unwrap();

    // Create two different clients with metadata indicating their group
    create_hydra_client_with_metadata(
        &hydra,
        "premium-client",
        json!({"group": "premium", "user_type": "enterprise"}),
    )
    .await
    .unwrap();

    create_hydra_client_with_metadata(
        &hydra,
        "basic-client",
        json!({"group": "basic", "user_type": "starter"}),
    )
    .await
    .unwrap();

    // Setup OpenAI mock
    let openai = OpenAIMock::new("openai").with_models(vec!["gpt-4".to_string()]);

    let mut builder = TestServer::builder();
    builder.spawn_llm(openai).await;

    // Configure server to use client_id from JWT as the identifier
    let config = indoc! {r#"
        [server.oauth]
        url = "http://127.0.0.1:4444/.well-known/jwks.json"
        poll_interval = "5m"
        expected_issuer = "http://127.0.0.1:4444"

        [server.oauth.protected_resource]
        resource = "http://127.0.0.1:8080"
        authorization_servers = ["http://127.0.0.1:4444"]

        [server.client_identification]
        enabled = true
        client_id = { jwt_claim = "client_id" }  # Use client_id claim

        [llm.providers.openai.rate_limits.per_user]
        limit = 100
        interval = "60s"
    "#};

    let server = builder.build(config).await;
    let client = &server.client;

    // Get tokens for both clients
    let premium_token = hydra
        .get_token("premium-client", "premium-client-secret")
        .await
        .unwrap();
    let basic_token = hydra.get_token("basic-client", "basic-client-secret").await.unwrap();

    // Test that different clients have independent rate limits
    let request = json!({
        "model": "openai/gpt-4",
        "messages": [{"role": "user", "content": "Test"}],
        "max_tokens": 80  // ~88 tokens total
    });

    // Premium client uses most of its limit
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&premium_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "Premium client first request should succeed");

    // Premium client hits rate limit
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&premium_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Premium client should hit rate limit");

    // Basic client should still be able to make requests (independent rate limit)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&basic_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "Basic client should have independent rate limit"
    );

    // Basic client also hits its limit
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&basic_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Basic client should also hit rate limit");
}

/// Test using sub claim (which equals client_id) for rate limiting.
#[tokio::test]
async fn test_rate_limiting_with_sub_claim() {
    let hydra = HydraClient::new(4444, 4445);
    hydra.wait_for_hydra().await.unwrap();

    // Create a client representing a user
    create_hydra_client_with_metadata(
        &hydra,
        "user-alice-123",
        json!({"real_user_id": "alice", "department": "engineering"}),
    )
    .await
    .unwrap();

    // Setup OpenAI mock
    let openai = OpenAIMock::new("openai").with_models(vec!["gpt-4".to_string()]);

    let mut builder = TestServer::builder();
    builder.spawn_llm(openai).await;

    // Configure server to use sub claim (which is client_id) as user identifier
    let config = indoc! {r#"
        [server.oauth]
        url = "http://127.0.0.1:4444/.well-known/jwks.json"
        poll_interval = "5m"
        expected_issuer = "http://127.0.0.1:4444"

        [server.oauth.protected_resource]
        resource = "http://127.0.0.1:8080"
        authorization_servers = ["http://127.0.0.1:4444"]

        [server.client_identification]
        enabled = true
        client_id = { jwt_claim = "sub" }  # Use sub claim (equals client_id)

        [llm.providers.openai.rate_limits.per_user]
        limit = 50
        interval = "60s"
    "#};

    let server = builder.build(config).await;
    let client = &server.client;

    // Get token for user alice
    let alice_token = hydra
        .get_token("user-alice-123", "user-alice-123-secret")
        .await
        .unwrap();

    // Test rate limiting for alice
    let request = json!({
        "model": "openai/gpt-4",
        "messages": [{"role": "user", "content": "Hi"}],
        "max_tokens": 40  // ~46 tokens total
    });

    // First request should succeed
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&alice_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "Alice's first request should succeed");

    // Second request should fail (exceeds 50 token limit)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&alice_token.access_token)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Alice should hit 50 token rate limit");

    let body = response.json::<serde_json::Value>().await.unwrap();
    insta::assert_json_snapshot!(body, {
        ".error.message" => "[rate limit message]"
    }, @r#"
    {
      "error": {
        "message": "[rate limit message]",
        "type": "rate_limit_error",
        "code": 429
      }
    }
    "#);
}

/// Test that two different JWT users have independent rate limits.
/// This demonstrates one user hitting their limit while another can still make requests.
#[tokio::test]
async fn two_jwt_users_independent_rate_limits() {
    let hydra = HydraClient::new(4444, 4445);
    hydra.wait_for_hydra().await.unwrap();

    // Create two different clients representing different users
    create_hydra_client_with_metadata(
        &hydra,
        "user-alice",
        json!({"user_type": "developer", "team": "backend"}),
    )
    .await
    .unwrap();

    create_hydra_client_with_metadata(
        &hydra,
        "user-bob",
        json!({"user_type": "developer", "team": "frontend"}),
    )
    .await
    .unwrap();

    // Setup OpenAI mock
    let openai = OpenAIMock::new("openai").with_models(vec!["gpt-4".to_string()]);

    let mut builder = TestServer::builder();
    builder.spawn_llm(openai).await;

    // Configure server with low rate limit for clear demonstration
    let config = indoc! {r#"
        [server.oauth]
        url = "http://127.0.0.1:4444/.well-known/jwks.json"
        poll_interval = "5m"
        expected_issuer = "http://127.0.0.1:4444"

        [server.oauth.protected_resource]
        resource = "http://127.0.0.1:8080"
        authorization_servers = ["http://127.0.0.1:4444"]

        [server.client_identification]
        enabled = true
        client_id = { jwt_claim = "sub" }  # Use sub claim (equals client_id)

        [llm.providers.openai.rate_limits.per_user]
        limit = 50  # Low limit for clear demonstration
        interval = "60s"
    "#};

    let server = builder.build(config).await;
    let client = &server.client;

    // Get tokens for both users
    let alice_token = hydra.get_token("user-alice", "user-alice-secret").await.unwrap();
    let bob_token = hydra.get_token("user-bob", "user-bob-secret").await.unwrap();

    // Request that uses most of the 50 token limit
    let large_request = json!({
        "model": "openai/gpt-4",
        "messages": [{"role": "user", "content": "Hi"}],
        "max_tokens": 40  // ~46 tokens total
    });

    // Small request that fits in remaining quota
    let small_request = json!({
        "model": "openai/gpt-4",
        "messages": [{"role": "user", "content": "Hi"}],
        "max_tokens": 10  // ~16 tokens total
    });

    // Alice uses her rate limit
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&alice_token.access_token)
        .json(&large_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "Alice's first request should succeed");

    // Alice hits her rate limit on second request (46 + 46 = 92 > 50)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&alice_token.access_token)
        .json(&large_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Alice should hit her rate limit");

    // Verify Alice is rate limited with proper error
    let body = response.json::<serde_json::Value>().await.unwrap();
    insta::assert_json_snapshot!(body, {
        ".error.message" => "[rate limit message]"
    }, @r#"
    {
      "error": {
        "message": "[rate limit message]",
        "type": "rate_limit_error",
        "code": 429
      }
    }
    "#);

    // Bob should still be able to make requests (independent rate limit)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&bob_token.access_token)
        .json(&small_request) // Small request (16 tokens)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "Bob should have independent rate limit and succeed"
    );

    // Bob can make another small request (16 + 16 = 32 < 50)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&bob_token.access_token)
        .json(&small_request) // Another small request
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "Bob's second request should also succeed");

    // But Alice is still rate limited
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&alice_token.access_token)
        .json(&small_request) // Even small request fails for Alice
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Alice should still be rate limited");

    // Eventually Bob will also hit his limit with a large request (32 + 46 = 78 > 50)
    let response = client
        .request(reqwest::Method::POST, "/llm/v1/chat/completions")
        .authorization(&bob_token.access_token)
        .json(&large_request) // Large request exceeds remaining quota
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429, "Bob should also hit his rate limit eventually");
}
