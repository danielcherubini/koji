use serde::{Deserialize, Serialize};
use strum::{Display, EnumIs, EnumString};

// ─── Re-exports will be added to mod.rs once types are implemented ───

/// How the provider is deployed
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    #[default]
    Local, // Managed by tamad
    Remote, // Direct HTTP endpoint
}

/// The underlying inference engine
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumIs, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    // Local engines (tamad-managed)
    LlamaCpp,
    IkLlama,
    TtsKokoro,
    Compaction,
    Docker,
    Custom,
    // Remote engines (direct HTTP)
    #[strum(serialize = "openai")]
    #[serde(rename = "openai")]
    OpenAI, // OpenAI-compatible (includes vLLM, llama.cpp API, etc.)
    Anthropic,
}

/// A registered provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: i64,
    pub name: String,
    pub provider_type: ProviderType,
    pub engine: Engine,
    /// For local: which tamad manages this provider
    pub tamad_id: Option<String>,
    /// For remote: base URL of the API
    pub base_url: Option<String>,
    /// For remote: API key (stored encrypted in DB)
    pub api_key: Option<String>,
    pub created_at: i64,
}

/// A registered tamad connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamadConnection {
    pub id: String,         // stable identifier (UUID)
    pub name: String,       // display name
    pub url: String,        // "grpc://..." or "http://..."
    pub protocol: Protocol, // "grpc" | "http"
    pub token: Option<String>,
    pub status: TamadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Grpc,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TamadStatus {
    Online,
    Offline,
    Unknown,
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    // ── Engine Display / FromStr roundtrip ──

    #[test]
    fn test_openai_displays_as_openai() {
        assert_eq!(Engine::OpenAI.to_string(), "openai");
    }

    #[test]
    fn test_openai_parses_from_openai() {
        let engine = Engine::from_str("openai").expect("openai should parse");
        assert!(matches!(engine, Engine::OpenAI));
    }

    #[test]
    fn test_openai_serializes_as_openai() {
        let json = serde_json::to_string(&Engine::OpenAI).expect("serialize");
        assert_eq!(json, "\"openai\"");
    }

    #[test]
    fn test_openai_deserializes_from_openai() {
        let engine: Engine = serde_json::from_str("\"openai\"").expect("deserialize");
        assert!(matches!(engine, Engine::OpenAI));
    }

    #[test]
    fn test_engine_local_variants_roundtrip() {
        // llama_cpp
        assert_eq!(Engine::LlamaCpp.to_string(), "llama_cpp");
        assert!(Engine::from_str("llama_cpp").unwrap().is_llama_cpp());

        // ik_llama
        assert_eq!(Engine::IkLlama.to_string(), "ik_llama");
        assert!(Engine::from_str("ik_llama").unwrap().is_ik_llama());

        // tts_kokoro
        assert_eq!(Engine::TtsKokoro.to_string(), "tts_kokoro");
        assert!(Engine::from_str("tts_kokoro").unwrap().is_tts_kokoro());

        // compaction
        assert_eq!(Engine::Compaction.to_string(), "compaction");
        assert!(Engine::from_str("compaction").unwrap().is_compaction());

        // docker
        assert_eq!(Engine::Docker.to_string(), "docker");
        assert!(Engine::from_str("docker").unwrap().is_docker());

        // custom
        assert_eq!(Engine::Custom.to_string(), "custom");
        assert!(Engine::from_str("custom").unwrap().is_custom());
    }

    #[test]
    fn test_engine_remote_variants_roundtrip() {
        // anthropic
        assert_eq!(Engine::Anthropic.to_string(), "anthropic");
        assert!(Engine::from_str("anthropic").unwrap().is_anthropic());
    }

    #[test]
    fn test_engine_is_methods() {
        let engine = Engine::OpenAI;
        assert!(engine.is_open_ai());
        assert!(!engine.is_llama_cpp());
        assert!(!engine.is_anthropic());

        let engine = Engine::LlamaCpp;
        assert!(engine.is_llama_cpp());
        assert!(!engine.is_open_ai());
    }

    // ── ProviderType roundtrip ──

    #[test]
    fn test_provider_type_roundtrip() {
        assert_eq!(ProviderType::Local.to_string(), "local");
        assert!(ProviderType::from_str("local").unwrap().is_local());
        assert!(ProviderType::from_str("local").unwrap().is_local());

        assert_eq!(ProviderType::Remote.to_string(), "remote");
        assert!(ProviderType::from_str("remote").unwrap().is_remote());
    }

    #[test]
    fn test_provider_type_default_is_local() {
        let default = ProviderType::default();
        assert!(default.is_local());
    }

    // ── Protocol roundtrip ──

    #[test]
    fn test_protocol_roundtrip() {
        assert_eq!(Protocol::Grpc.to_string(), "grpc");
        assert!(Protocol::from_str("grpc").unwrap().is_grpc());

        assert_eq!(Protocol::Http.to_string(), "http");
        assert!(Protocol::from_str("http").unwrap().is_http());
    }

    // ── TamadStatus roundtrip ──

    #[test]
    fn test_tamad_status_roundtrip() {
        assert_eq!(TamadStatus::Online.to_string(), "online");
        assert!(TamadStatus::from_str("online").unwrap().is_online());

        assert_eq!(TamadStatus::Offline.to_string(), "offline");
        assert!(TamadStatus::from_str("offline").unwrap().is_offline());

        assert_eq!(TamadStatus::Unknown.to_string(), "unknown");
        assert!(TamadStatus::from_str("unknown").unwrap().is_unknown());
    }

    // ── Provider serialization ──

    #[test]
    fn test_provider_serialize_deserialize() {
        let provider = Provider {
            id: 1,
            name: "My Provider".to_string(),
            provider_type: ProviderType::Remote,
            engine: Engine::OpenAI,
            tamad_id: None,
            base_url: Some("https://api.openai.com/v1".to_string()),
            api_key: Some("sk-xxx".to_string()),
            created_at: 1700000000,
        };

        let json = serde_json::to_string(&provider).expect("serialize");
        let deserialized: Provider = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.id, provider.id);
        assert_eq!(deserialized.name, provider.name);
        assert!(deserialized.provider_type.is_remote());
        assert!(deserialized.engine.is_open_ai());
        assert_eq!(
            deserialized.base_url,
            Some("https://api.openai.com/v1".to_string())
        );
    }

    // ── TamadConnection serialization ──

    #[test]
    fn test_tamad_connection_serialize_deserialize() {
        let conn = TamadConnection {
            id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            name: "Local Tamad".to_string(),
            url: "grpc://localhost:50051".to_string(),
            protocol: Protocol::Grpc,
            token: Some("my-token".to_string()),
            status: TamadStatus::Online,
        };

        let json = serde_json::to_string(&conn).expect("serialize");
        let deserialized: TamadConnection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.id, conn.id);
        assert_eq!(deserialized.name, conn.name);
        assert!(deserialized.protocol.is_grpc());
        assert!(deserialized.status.is_online());
    }
}
