//! Decision models: the gate's classifier for actions no rule covers. A decision model answers
//! typed questions with probabilities and writes no text. The builder connects one with an API
//! key, which the vault holds; the daemon's settings name which one and the key's reference.

use serde::{Deserialize, Serialize};

use crate::id::CredentialRef;

/// The three the owner chose. Each takes an API key only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProvider {
    /// Google's Gemini Decision 1, on the Gemini Developer API.
    Gemini,
    /// OpenAI's Decisions API.
    #[serde(rename = "openai")]
    OpenAi,
    /// TypeSafe AI's Jev.
    Jev,
}

impl DecisionProvider {
    pub const ALL: [DecisionProvider; 3] = [
        DecisionProvider::Gemini,
        DecisionProvider::OpenAi,
        DecisionProvider::Jev,
    ];

    /// The one host its key may be sent to. A connected key's credential is scoped to it alone.
    pub fn host(self) -> &'static str {
        match self {
            DecisionProvider::Gemini => "generativelanguage.googleapis.com",
            DecisionProvider::OpenAi => "api.openai.com",
            DecisionProvider::Jev => "api.typesafe.ai",
        }
    }
}

/// The decision model that classifies, and the credential that holds its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DecisionModel {
    pub provider: DecisionProvider,
    pub credential: CredentialRef,
}
