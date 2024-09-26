use gateway::embeddings::{EmbeddingProvider, EmbeddingProviderConfig, EmbeddingRequest};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    common::{
        get_clickhouse, get_gateway_endpoint, select_chat_inference_clickhouse,
        select_model_inference_clickhouse,
    },
    providers::common::{E2ETestProvider, E2ETestProviders},
};

crate::generate_provider_tests!(get_providers);

async fn get_providers() -> E2ETestProviders {
    let standard_providers = vec![E2ETestProvider {
        variant_name: "openai".to_string(),
        model_name: "gpt-4o-mini-2024-07-18".to_string(),
        model_provider_name: "openai".to_string(),
    }];

    let json_providers = vec![
        E2ETestProvider {
            variant_name: "openai".to_string(),
            model_name: "gpt-4o-mini-2024-07-18".to_string(),
            model_provider_name: "openai".to_string(),
        },
        E2ETestProvider {
            variant_name: "openai-implicit".to_string(),
            model_name: "gpt-4o-mini-2024-07-18".to_string(),
            model_provider_name: "openai".to_string(),
        },
        E2ETestProvider {
            variant_name: "openai-strict".to_string(),
            model_name: "gpt-4o-mini-2024-07-18".to_string(),
            model_provider_name: "openai".to_string(),
        },
    ];

    E2ETestProviders {
        simple_inference: standard_providers.clone(),
        inference_params_inference: standard_providers.clone(),
        tool_use_inference: standard_providers.clone(),
        tool_multi_turn_inference: standard_providers.clone(),
        dynamic_tool_use_inference: standard_providers.clone(),
        parallel_tool_use_inference: standard_providers.clone(),
        json_mode_inference: json_providers.clone(),
    }
}

#[tokio::test]
async fn test_o1_mini_inference() {
    let client = Client::new();
    let episode_id = Uuid::now_v7();

    let payload = json!({
        "function_name": "basic_test",
        "variant_name": "o1-mini",
        "episode_id": episode_id,
        "input":
            {"system": {"assistant_name": "AskJeeves"},
            "messages": [
                {
                    "role": "user",
                    "content": "What is the capital of Japan?"
                }
            ]},
        "stream": false,
    });

    let response = client
        .post(get_gateway_endpoint("/inference"))
        .json(&payload)
        .send()
        .await
        .unwrap();
    // Check Response is OK, then fields in order
    assert_eq!(response.status(), StatusCode::OK);
    let response_json = response.json::<Value>().await.unwrap();
    let content_blocks = response_json.get("content").unwrap().as_array().unwrap();
    assert!(content_blocks.len() == 1);
    let content_block = content_blocks.first().unwrap();
    let content_block_type = content_block.get("type").unwrap().as_str().unwrap();
    assert_eq!(content_block_type, "text");
    let content = content_block.get("text").unwrap().as_str().unwrap();
    // Assert that Tokyo is in the content
    assert!(content.contains("Tokyo"), "Content should mention Tokyo");
    // Check that inference_id is here
    let inference_id = response_json.get("inference_id").unwrap().as_str().unwrap();
    let inference_id = Uuid::parse_str(inference_id).unwrap();

    // Sleep for 1 second to allow time for data to be inserted into ClickHouse (trailing writes from API)
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Check ClickHouse
    let clickhouse = get_clickhouse().await;

    // First, check Inference table
    let result = select_chat_inference_clickhouse(&clickhouse, inference_id)
        .await
        .unwrap();
    let id = result.get("id").unwrap().as_str().unwrap();
    let id_uuid = Uuid::parse_str(id).unwrap();
    assert_eq!(id_uuid, inference_id);
    let input: Value =
        serde_json::from_str(result.get("input").unwrap().as_str().unwrap()).unwrap();
    let correct_input = json!({
        "system": {"assistant_name": "AskJeeves"},
        "messages": [
            {
                "role": "user",
                "content": [{"type": "text", "value": "What is the capital of Japan?"}]
            }
        ]
    });
    assert_eq!(input, correct_input);
    let content_blocks = result.get("output").unwrap().as_str().unwrap();
    // Check that content_blocks is a list of blocks length 1
    let content_blocks: Vec<Value> = serde_json::from_str(content_blocks).unwrap();
    assert_eq!(content_blocks.len(), 1);
    let content_block = content_blocks.first().unwrap();
    // Check the type and content in the block
    let content_block_type = content_block.get("type").unwrap().as_str().unwrap();
    assert_eq!(content_block_type, "text");
    let clickhouse_content = content_block.get("text").unwrap().as_str().unwrap();
    assert_eq!(clickhouse_content, content);
    // Check that episode_id is here and correct
    let retrieved_episode_id = result.get("episode_id").unwrap().as_str().unwrap();
    let retrieved_episode_id = Uuid::parse_str(retrieved_episode_id).unwrap();
    assert_eq!(retrieved_episode_id, episode_id);
    // Check the variant name
    let variant_name = result.get("variant_name").unwrap().as_str().unwrap();
    assert_eq!(variant_name, "o1-mini");
    // Check the processing time
    let processing_time_ms = result.get("processing_time_ms").unwrap().as_u64().unwrap();
    assert!(processing_time_ms > 0);

    // Check the ModelInference Table
    let result = select_model_inference_clickhouse(&clickhouse, inference_id)
        .await
        .unwrap();
    let inference_id_result = result.get("inference_id").unwrap().as_str().unwrap();
    let inference_id_result = Uuid::parse_str(inference_id_result).unwrap();
    assert_eq!(inference_id_result, inference_id);
    let model_name = result.get("model_name").unwrap().as_str().unwrap();
    assert_eq!(model_name, "o1-mini");
    let model_provider_name = result.get("model_provider_name").unwrap().as_str().unwrap();
    assert_eq!(model_provider_name, "openai");
    let raw_request = result.get("raw_request").unwrap().as_str().unwrap();
    assert!(raw_request.to_lowercase().contains("japan"));
    // Check that raw_request is valid JSON
    let _: Value = serde_json::from_str(raw_request).expect("raw_request should be valid JSON");
    let input_tokens = result.get("input_tokens").unwrap().as_u64().unwrap();
    assert!(input_tokens > 5);
    let output_tokens = result.get("output_tokens").unwrap().as_u64().unwrap();
    assert!(output_tokens > 5);
    let response_time_ms = result.get("response_time_ms").unwrap().as_u64().unwrap();
    assert!(response_time_ms > 0);
    assert!(result.get("ttft_ms").unwrap().is_null());
    let raw_response = result.get("raw_response").unwrap().as_str().unwrap();
    let _raw_response_json: Value = serde_json::from_str(raw_response).unwrap();
}

#[tokio::test]
async fn test_embedding_request() {
    let provider_config_serialized = r#"
    type = "openai"
    model_name = "text-embedding-3-small"
    "#;
    let provider_config: EmbeddingProviderConfig = toml::from_str(provider_config_serialized)
        .expect("Failed to deserialize EmbeddingProviderConfig");
    assert!(matches!(
        provider_config,
        EmbeddingProviderConfig::OpenAI(_)
    ));

    let client = Client::new();
    let request = EmbeddingRequest {
        input: "This is a test input".to_string(),
    };
    let response = provider_config.embed(&request, &client).await.unwrap();
    assert_eq!(response.embedding.len(), 1536);
}
