use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizHints {
    #[serde(default)]
    pub quiz_urls: Vec<String>,
    #[serde(default)]
    pub assessment_urns: Vec<String>,
}

pub fn quiz_hints_from_json(value: &str) -> QuizHints {
    serde_json::from_str(value).unwrap_or(QuizHints {
        quiz_urls: Vec::new(),
        assessment_urns: Vec::new(),
    })
}

pub fn quiz_hints_json(hints: &QuizHints) -> String {
    serde_json::to_string(hints).unwrap_or_else(|_| "[]".to_string())
}
