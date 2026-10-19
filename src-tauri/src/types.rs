use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments, exactly as the model produced them.
    pub arguments: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

fn function_kind() -> String {
    "function".to_string()
}

fn assistant_role() -> String {
    "assistant".to_string()
}

/// One message in an OpenAI-style conversation. `content` is null on an
/// assistant message that only carries `tool_calls`.
#[derive(Deserialize, Clone, Debug)]
pub struct ChatMessage {
    #[serde(default = "assistant_role")]
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Images the user attached, as `data:image/...;base64,...` URLs. Never
    /// stored: they live in memory for the conversation and are sent to the
    /// model as image parts of this message.
    #[serde(default, skip)]
    pub images: Vec<String>,
}

impl Serialize for ChatMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;
        if self.images.is_empty() {
            map.serialize_entry("content", &self.content)?;
        } else {
            // OpenAI-style multimodal content: the text, then each image.
            let mut parts = Vec::with_capacity(self.images.len() + 1);
            if let Some(text) = self.content.as_deref().filter(|t| !t.is_empty()) {
                parts.push(serde_json::json!({ "type": "text", "text": text }));
            }
            for url in &self.images {
                parts.push(serde_json::json!({ "type": "image_url", "image_url": { "url": url } }));
            }
            map.serialize_entry("content", &parts)?;
        }
        if let Some(calls) = &self.tool_calls {
            map.serialize_entry("tool_calls", calls)?;
        }
        if let Some(id) = &self.tool_call_id {
            map.serialize_entry("tool_call_id", id)?;
        }
        map.end()
    }
}

impl ChatMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            images: Vec::new(),
        }
    }

    pub fn tool_result(call_id: &str, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            images: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_messages_serialize_as_before() {
        let v = serde_json::to_value(ChatMessage::text("user", "hi")).unwrap();
        assert_eq!(v, serde_json::json!({ "role": "user", "content": "hi" }));
    }

    #[test]
    fn images_become_content_parts() {
        let mut m = ChatMessage::text("user", "what is this?");
        m.images = vec!["data:image/jpeg;base64,AAAA".to_string()];
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][1]["type"], "image_url");
        assert_eq!(v["content"][1]["image_url"]["url"], "data:image/jpeg;base64,AAAA");
    }
}
