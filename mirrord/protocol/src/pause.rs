use bincode::{Decode, Encode};

/// `-agent` --> `-layer` messages regarding the pause feature.
#[derive(Encode, Decode, Debug, PartialEq, Eq, Clone)]
pub enum DaemonPauseTarget {
    /// Response for the client's request to pause or unpause the container.
    PauseResponse {
        /// The container changed its state.
        changed: bool,
        /// Current state of the container.
        container_paused: bool,
    },
}
