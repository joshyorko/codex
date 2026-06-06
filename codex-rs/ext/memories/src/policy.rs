use serde_json::Value;
use serde_json::json;

use crate::portable_schema::PortableMemorySettings;

pub(crate) const MAX_WRITEBACK_CHARS: usize = 24_000;

pub(crate) fn sanitize_visible_memory_content(content: &str) -> Option<String> {
    let content = content.trim();
    if content.is_empty() || contains_blocked_memory_content(content) {
        return None;
    }
    Some(truncate_chars(content, MAX_WRITEBACK_CHARS))
}

fn contains_blocked_memory_content(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("secret=")
        || lower.contains("password=")
        || lower.contains("private key")
        || lower.contains("begin openssh private key")
        || lower.contains("begin rsa private key")
        || lower.contains(".env")
        || lower.contains("<codex_internal_context")
        || content.contains("sk-")
        || content.contains("hch-v")
}

pub(crate) fn portable_metadata(
    settings: &PortableMemorySettings,
    origin: &str,
    provenance: &str,
    sensitivity: &str,
) -> Value {
    json!({
        "origin": origin,
        "profile": settings.profile.as_str(),
        "workspace": settings.workspace.as_str(),
        "repo": Value::Null,
        "sensitivity": sensitivity,
        "portability": "portable",
        "provenance": provenance,
        "confidence": "observed",
    })
}

pub(crate) fn truncate_chars(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n[truncated by Codex portable memory]");
    truncated
}
