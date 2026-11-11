//! The access token as a command argument: a string that never prints itself.

use drafftink_collab::{AuthToken, ConfigError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An access token as it arrives in the arguments of `collab.connect` and `collab.join_room`. `Debug` prints a
/// placeholder, so a logged argument struct cannot leak it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TokenArg(String);

impl std::fmt::Debug for TokenArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenArg(<hidden>)")
    }
}

impl TokenArg {
    /// Checks the token against the protocol's limits. The error text never holds the token.
    pub(super) fn checked(&self) -> Result<AuthToken, ConfigError> {
        AuthToken::new(self.0.as_str())
    }
}
